// Copyright (C) 2021 Subspace Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Common primitives for subspace domain runtime.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::string::String;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
pub use fp_account::AccountId20;
use frame_support::dispatch::{DispatchClass, PerDispatchClass};
use frame_support::weights::constants::{BlockExecutionWeight, ExtrinsicBaseWeight};
use frame_system::limits::{BlockLength, BlockWeights};
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
use serde::{Deserialize, Serialize};
use sp_runtime::generic::UncheckedExtrinsic;
use sp_runtime::traits::{Convert, IdentifyAccount, Verify};
use sp_runtime::transaction_validity::TransactionValidityError;
use sp_runtime::{MultiAddress, MultiSignature, Perbill};
use sp_weights::Weight;
use subspace_runtime_primitives::SHANNON;

/// Alias to 512-bit hash when used in the context of a transaction signature on the chain.
pub type Signature = MultiSignature;

/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// Balance of an account.
pub type Balance = u128;

/// Index of a transaction in the chain.
pub type Nonce = u32;

/// A hash of some data used by the chain.
pub type Hash = sp_core::H256;

/// An index to a block.
pub type BlockNumber = u32;

/// The address format for describing accounts.
pub type Address = MultiAddress<AccountId, ()>;

/// Slot duration that is same as consensus chain runtime.
pub const SLOT_DURATION: u64 = 1000;

/// The EVM chain Id type
pub type EVMChainId = u64;

/// Maximum block length for mandatory dispatch.
pub const MAXIMUM_MANDATORY_BLOCK_LENGTH: u32 = 5 * 1024 * 1024;

/// Maximum block length for operational and normal dispatches.
pub const MAXIMUM_OPERATIONAL_AND_NORMAL_BLOCK_LENGTH: u32 = u32::MAX;

/// Maximum block length for all dispatches.
pub fn maximum_block_length() -> BlockLength {
    BlockLength {
        max: PerDispatchClass::new(|class| match class {
            DispatchClass::Normal | DispatchClass::Operational => {
                MAXIMUM_OPERATIONAL_AND_NORMAL_BLOCK_LENGTH
            }
            DispatchClass::Mandatory => MAXIMUM_MANDATORY_BLOCK_LENGTH,
        }),
    }
}

/// The existential deposit. Same with the one on primary chain.
pub const EXISTENTIAL_DEPOSIT: Balance = 500 * SHANNON;

/// We assume that ~5% of the block weight is consumed by `on_initialize` handlers. This is
/// used to limit the maximal weight of a single extrinsic.
const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(5);

/// Maximum total block weight.
pub const MAXIMUM_BLOCK_WEIGHT: Weight = Weight::from_parts(u64::MAX, u64::MAX);

pub fn block_weights() -> BlockWeights {
    BlockWeights::builder()
        .base_block(BlockExecutionWeight::get())
        .for_class(DispatchClass::all(), |weights| {
            weights.base_extrinsic = ExtrinsicBaseWeight::get();
        })
        .for_class(DispatchClass::Normal, |weights| {
            // explicitly set max_total weight for normal dispatches to maximum
            weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
        })
        .for_class(DispatchClass::Operational, |weights| {
            // explicitly set max_total weight for operational dispatches to maximum
            weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
        })
        .avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
        .build_or_panic()
}

/// Extracts the signer from an unchecked extrinsic.
///
/// Used by executor to extract the optional signer when shuffling the extrinsics.
pub trait Signer<AccountId, Lookup> {
    /// Returns the AccountId of signer.
    fn signer(&self, lookup: &Lookup) -> Option<AccountId>;
}

impl<Address, AccountId, Call, Signature, Extra, Lookup> Signer<AccountId, Lookup>
    for UncheckedExtrinsic<Address, Call, Signature, Extra>
where
    Address: Clone,
    Extra: sp_runtime::traits::SignedExtension<AccountId = AccountId>,
    Lookup: sp_runtime::traits::Lookup<Source = Address, Target = AccountId>,
{
    fn signer(&self, lookup: &Lookup) -> Option<AccountId> {
        self.signature
            .as_ref()
            .and_then(|(signed, _, _)| lookup.lookup(signed.clone()).ok())
    }
}

/// MultiAccountId used by all the domains to describe their account type.
#[derive(
    Debug, Encode, Decode, Clone, Eq, PartialEq, TypeInfo, Serialize, Deserialize, Ord, PartialOrd,
)]
pub enum MultiAccountId {
    /// 32 byte account Id.
    AccountId32([u8; 32]),
    /// 20 byte account Id. Ex: Ethereum
    AccountId20([u8; 20]),
    /// Some raw bytes
    Raw(Vec<u8>),
}

/// Extensible conversion trait. Generic over both source and destination types.
pub trait TryConvertBack<A, B>: Convert<A, B> {
    /// Make conversion back.
    fn try_convert_back(b: B) -> Option<A>;
}

/// An AccountId32 to MultiAccount converter.
pub struct AccountIdConverter;

impl Convert<AccountId, MultiAccountId> for AccountIdConverter {
    fn convert(account_id: AccountId) -> MultiAccountId {
        MultiAccountId::AccountId32(account_id.into())
    }
}

impl TryConvertBack<AccountId, MultiAccountId> for AccountIdConverter {
    fn try_convert_back(multi_account_id: MultiAccountId) -> Option<AccountId> {
        match multi_account_id {
            MultiAccountId::AccountId32(acc) => Some(AccountId::new(acc)),
            _ => None,
        }
    }
}

/// An AccountId20 to MultiAccount converter.
pub struct AccountId20Converter;

impl Convert<AccountId20, MultiAccountId> for AccountId20Converter {
    fn convert(account_id: AccountId20) -> MultiAccountId {
        MultiAccountId::AccountId20(account_id.into())
    }
}

impl TryConvertBack<AccountId20, MultiAccountId> for AccountId20Converter {
    fn try_convert_back(multi_account_id: MultiAccountId) -> Option<AccountId20> {
        match multi_account_id {
            MultiAccountId::AccountId20(acc) => Some(AccountId20::from(acc)),
            _ => None,
        }
    }
}

#[derive(Debug, Decode, Encode, TypeInfo, PartialEq, Eq, Clone)]
pub struct CheckExtrinsicsValidityError {
    pub extrinsic_index: u32,
    pub transaction_validity_error: TransactionValidityError,
}

#[derive(Debug, Decode, Encode, TypeInfo, PartialEq, Eq, Clone)]
pub struct DecodeExtrinsicError(pub String);

/// fullu qualified method name of check_extrinsics_and_do_pre_dispatch runtime api.
/// Used to call state machine.
/// Change it when the runtime api's name is changed in the interface.
pub const CHECK_EXTRINSICS_AND_DO_PRE_DISPATCH_METHOD_NAME: &str =
    "DomainCoreApi_check_extrinsics_and_do_pre_dispatch";

/// Opaque types. These are used by the CLI to instantiate machinery that don't need to know
/// the specifics of the runtime. They can then be made to be agnostic over specific formats
/// of data like extrinsics, allowing for them to continue syncing the network through upgrades
/// to even the core data structures.
pub mod opaque {
    use crate::BlockNumber;
    #[cfg(not(feature = "std"))]
    use alloc::vec::Vec;
    use sp_runtime::generic;
    use sp_runtime::traits::BlakeTwo256;
    pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;

    /// Opaque block header type.
    pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
    /// Opaque block type.
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;
    /// Opaque block identifier type.
    pub type BlockId = generic::BlockId<Block>;
    /// Opaque account identifier type.
    pub type AccountId = Vec<u8>;
}

#[cfg(test)]
mod test {
    use super::block_weights;

    #[test]
    fn test_block_weights() {
        // validate and build block weights
        let _block_weights = block_weights();
    }
}
