use crate::single_disk_plot::{Handlers, PlotMetadataHeader, RESERVED_PLOT_METADATA};
use crate::{node_client, NodeClient};
use memmap2::{MmapMut, MmapOptions};
use parity_scale_codec::Encode;
use parking_lot::RwLock;
use std::fs::File;
use std::io;
use std::num::NonZeroU16;
use std::sync::Arc;
use subspace_core_primitives::crypto::kzg::Kzg;
use subspace_core_primitives::{PublicKey, SectorIndex};
use subspace_erasure_coding::ErasureCoding;
use subspace_farmer_components::piece_caching::PieceMemoryCache;
use subspace_farmer_components::plotting;
use subspace_farmer_components::plotting::{plot_sector, PieceGetter, PieceGetterRetryPolicy};
use subspace_farmer_components::sector::SectorMetadata;
use subspace_proof_of_space::Table;
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::{debug, info, trace, warn};

/// Get piece retry attempts number.
const PIECE_GETTER_RETRY_NUMBER: NonZeroU16 = NonZeroU16::new(3).expect("Not zero; qed");

/// Errors that happen during plotting
#[derive(Debug, Error)]
pub enum PlottingError {
    /// Failed to retrieve farmer info
    #[error("Failed to retrieve farmer info: {error}")]
    FailedToGetFarmerInfo {
        /// Lower-level error
        error: node_client::Error,
    },
    /// I/O error occurred
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Low-level plotting error
    #[error("Low-level plotting error: {0}")]
    LowLevel(#[from] plotting::PlottingError),
}

/// Starts plotting process.
///
/// NOTE: Returned future is async, but does blocking operations and should be running in dedicated
/// thread.
#[allow(clippy::too_many_arguments)]
pub(super) async fn plotting<NC, PG, PosTable>(
    public_key: PublicKey,
    first_sector_index: SectorIndex,
    node_client: NC,
    pieces_in_sector: u16,
    sector_size: usize,
    sector_metadata_size: usize,
    target_sector_count: usize,
    mut metadata_header: PlotMetadataHeader,
    mut metadata_header_mmap: MmapMut,
    plot_file: Arc<File>,
    metadata_file: File,
    sectors_metadata: Arc<RwLock<Vec<SectorMetadata>>>,
    piece_getter: PG,
    piece_memory_cache: PieceMemoryCache,
    kzg: Kzg,
    erasure_coding: ErasureCoding,
    handlers: Arc<Handlers>,
    concurrent_plotting_semaphore: Arc<Semaphore>,
) -> Result<(), PlottingError>
where
    NC: NodeClient,
    PG: PieceGetter + Send + 'static,
    PosTable: Table,
{
    // Some sectors may already be plotted, skip them
    let sectors_offsets_left_to_plot = metadata_header.sector_count as usize..target_sector_count;

    // TODO: Concurrency
    for sector_offset in sectors_offsets_left_to_plot {
        let sector_index = sector_offset as u64 + first_sector_index;
        trace!(%sector_offset, %sector_index, "Preparing to plot sector");

        let mut sector = unsafe {
            MmapOptions::new()
                .offset((sector_offset * sector_size) as u64)
                .len(sector_size)
                .map_mut(&*plot_file)?
        };
        let mut sector_metadata = unsafe {
            MmapOptions::new()
                .offset(RESERVED_PLOT_METADATA + (sector_offset * sector_metadata_size) as u64)
                .len(sector_metadata_size)
                .map_mut(&metadata_file)?
        };
        let plotting_permit = match concurrent_plotting_semaphore.clone().acquire_owned().await {
            Ok(plotting_permit) => plotting_permit,
            Err(error) => {
                warn!(
                    %sector_offset,
                    %sector_index,
                    %error,
                    "Semaphore was closed, interrupting plotting"
                );
                return Ok(());
            }
        };

        debug!(%sector_offset, %sector_index, "Plotting sector");

        let farmer_app_info = node_client
            .farmer_app_info()
            .await
            .map_err(|error| PlottingError::FailedToGetFarmerInfo { error })?;

        let plot_sector_fut = plot_sector::<_, PosTable>(
            &public_key,
            sector_offset,
            sector_index,
            &piece_getter,
            PieceGetterRetryPolicy::Limited(PIECE_GETTER_RETRY_NUMBER.get()),
            &farmer_app_info.protocol_info,
            &kzg,
            &erasure_coding,
            pieces_in_sector,
            &mut sector,
            &mut sector_metadata,
            piece_memory_cache.clone(),
        );
        let plotted_sector = plot_sector_fut.await?;
        sector.flush()?;
        sector_metadata.flush()?;

        metadata_header.sector_count += 1;
        metadata_header_mmap.copy_from_slice(metadata_header.encode().as_slice());
        sectors_metadata
            .write()
            .push(plotted_sector.sector_metadata.clone());

        info!(%sector_offset, %sector_index, "Sector plotted successfully");

        handlers.sector_plotted.call_simple(&(
            sector_offset,
            plotted_sector,
            Arc::new(plotting_permit),
        ));
    }

    Ok(())
}