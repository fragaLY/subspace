use crate::mock::chain_a::{
    new_test_ext as new_chain_a_ext, Messenger, Runtime, RuntimeEvent, RuntimeOrigin, System,
};
use crate::mock::{
    chain_a, chain_b, storage_proof_of_inbox_message_responses, storage_proof_of_outbox_messages,
    AccountId, Balance, TestExternalities,
};
use crate::{
    Channel, ChannelId, ChannelState, Channels, Error, FeeModel, Inbox, InboxResponses, Nonce,
    Outbox, OutboxMessageResult, OutboxResponses, Pallet, U256,
};
use frame_support::{assert_err, assert_ok};
use pallet_transporter::Location;
use sp_core::storage::StorageKey;
use sp_core::{Blake2Hasher, H256};
use sp_domains::proof_provider_and_verifier::{StorageProofVerifier, VerificationError};
use sp_messenger::endpoint::{Endpoint, EndpointPayload, EndpointRequest, Sender};
use sp_messenger::messages::{
    ChainId, ConsensusChainMmrLeafProof, CrossDomainMessage, InitiateChannelParams,
    MessageWeightTag, Payload, Proof, ProtocolMessageRequest, RequestResponse, VersionedPayload,
};
use sp_mmr_primitives::{EncodableOpaqueLeaf, Proof as MmrProof};
use sp_runtime::traits::Convert;
use sp_trie::StorageProof;

fn create_channel(chain_id: ChainId, channel_id: ChannelId, fee_model: FeeModel<Balance>) {
    let params = InitiateChannelParams {
        max_outgoing_messages: 100,
        fee_model,
    };
    assert_ok!(Messenger::initiate_channel(
        RuntimeOrigin::root(),
        chain_id,
        params,
    ));

    System::assert_has_event(RuntimeEvent::Messenger(
        crate::Event::<Runtime>::ChannelInitiated {
            chain_id,
            channel_id,
        },
    ));
    assert_eq!(
        Messenger::next_channel_id(chain_id),
        channel_id.checked_add(U256::one()).unwrap()
    );

    let channel = Messenger::channels(chain_id, channel_id).unwrap();
    assert_eq!(channel.state, ChannelState::Initiated);
    assert_eq!(channel.next_inbox_nonce, Nonce::zero());
    assert_eq!(channel.next_outbox_nonce, Nonce::one());
    assert_eq!(channel.latest_response_received_message_nonce, None);
    assert_eq!(Outbox::<Runtime>::count(), 1);
    let msg = Outbox::<Runtime>::get((chain_id, channel_id, Nonce::zero())).unwrap();
    assert_eq!(msg.dst_chain_id, chain_id);
    assert_eq!(msg.channel_id, channel_id);
    assert_eq!(
        msg.payload,
        VersionedPayload::V0(Payload::Protocol(RequestResponse::Request(
            ProtocolMessageRequest::ChannelOpen(params)
        )))
    );

    System::assert_last_event(RuntimeEvent::Messenger(
        crate::Event::<Runtime>::OutboxMessage {
            chain_id,
            channel_id,
            nonce: Nonce::zero(),
        },
    ));

    // check outbox relayer storage key generation
    let messages_with_keys = chain_a::Messenger::get_block_messages();
    assert_eq!(messages_with_keys.outbox.len(), 1);
    assert_eq!(messages_with_keys.inbox_responses.len(), 0);
    let expected_key =
        Outbox::<chain_a::Runtime>::hashed_key_for((chain_id, channel_id, Nonce::zero()));
    assert_eq!(messages_with_keys.outbox[0].storage_key, expected_key);
}

fn default_consensus_proof() -> ConsensusChainMmrLeafProof<H256, H256> {
    ConsensusChainMmrLeafProof {
        consensus_block_hash: Default::default(),
        opaque_mmr_leaf: EncodableOpaqueLeaf(vec![]),
        proof: MmrProof {
            leaf_indices: vec![],
            leaf_count: 0,
            items: vec![],
        },
    }
}

fn close_channel(chain_id: ChainId, channel_id: ChannelId, last_delivered_nonce: Option<Nonce>) {
    assert_ok!(Messenger::close_channel(
        RuntimeOrigin::root(),
        chain_id,
        channel_id,
    ));

    let channel = Messenger::channels(chain_id, channel_id).unwrap();
    assert_eq!(channel.state, ChannelState::Closed);
    System::assert_has_event(RuntimeEvent::Messenger(
        crate::Event::<Runtime>::ChannelClosed {
            chain_id,
            channel_id,
        },
    ));

    let msg = Outbox::<Runtime>::get((chain_id, channel_id, Nonce::one())).unwrap();
    assert_eq!(msg.dst_chain_id, chain_id);
    assert_eq!(msg.channel_id, channel_id);
    assert_eq!(
        msg.last_delivered_message_response_nonce,
        last_delivered_nonce
    );
    assert_eq!(
        msg.payload,
        VersionedPayload::V0(Payload::Protocol(RequestResponse::Request(
            ProtocolMessageRequest::ChannelClose
        )))
    );

    System::assert_last_event(RuntimeEvent::Messenger(
        crate::Event::<Runtime>::OutboxMessage {
            chain_id,
            channel_id,
            nonce: Nonce::one(),
        },
    ));
}

#[test]
fn test_initiate_channel() {
    new_chain_a_ext().execute_with(|| {
        let chain_id = 2.into();
        let channel_id = U256::zero();
        create_channel(chain_id, channel_id, Default::default())
    });
}

#[test]
fn test_close_missing_channel() {
    new_chain_a_ext().execute_with(|| {
        let chain_id = 2.into();
        let channel_id = U256::zero();
        assert_err!(
            Messenger::close_channel(RuntimeOrigin::root(), chain_id, channel_id,),
            Error::<Runtime>::MissingChannel
        );
    });
}

#[test]
fn test_close_not_open_channel() {
    new_chain_a_ext().execute_with(|| {
        let chain_id = 2.into();
        let channel_id = U256::zero();
        create_channel(chain_id, channel_id, Default::default());
        assert_err!(
            Messenger::close_channel(RuntimeOrigin::root(), chain_id, channel_id,),
            Error::<Runtime>::InvalidChannelState
        );
    });
}

#[test]
fn test_close_open_channel() {
    new_chain_a_ext().execute_with(|| {
        let chain_id = 2.into();
        let channel_id = U256::zero();
        create_channel(chain_id, channel_id, Default::default());

        // open channel
        assert_ok!(Messenger::do_open_channel(chain_id, channel_id));
        let channel = Messenger::channels(chain_id, channel_id).unwrap();
        assert_eq!(channel.state, ChannelState::Open);
        System::assert_has_event(RuntimeEvent::Messenger(
            crate::Event::<Runtime>::ChannelOpen {
                chain_id,
                channel_id,
            },
        ));

        // close channel
        close_channel(chain_id, channel_id, None)
    });
}

#[test]
fn test_storage_proof_verification_invalid() {
    let mut t = new_chain_a_ext();
    let chain_id = 2.into();
    let channel_id = U256::zero();
    t.execute_with(|| {
        create_channel(chain_id, channel_id, Default::default());
        assert_ok!(Messenger::do_open_channel(chain_id, channel_id));
    });

    let (_, storage_key, storage_proof) =
        crate::mock::storage_proof_of_channels::<Runtime>(t.as_backend(), chain_id, channel_id);
    let res: Result<Channel<Balance>, VerificationError> =
        StorageProofVerifier::<Blake2Hasher>::get_decoded_value(
            &H256::zero(),
            storage_proof,
            storage_key,
        );
    assert_err!(res, VerificationError::InvalidProof);
}

#[test]
fn test_storage_proof_verification_missing_value() {
    let mut t = new_chain_a_ext();
    let chain_id = 2.into();
    let channel_id = U256::zero();
    t.execute_with(|| {
        create_channel(chain_id, channel_id, Default::default());
        assert_ok!(Messenger::do_open_channel(chain_id, channel_id));
    });

    let (state_root, _, storage_proof) =
        crate::mock::storage_proof_of_channels::<Runtime>(t.as_backend(), chain_id, U256::one());
    let res: Result<Channel<Balance>, VerificationError> =
        StorageProofVerifier::<Blake2Hasher>::get_decoded_value(
            &state_root,
            storage_proof,
            StorageKey(vec![]),
        );
    assert_err!(res, VerificationError::MissingValue);
}

#[test]
fn test_storage_proof_verification() {
    let mut t = new_chain_a_ext();
    let chain_id = 2.into();
    let channel_id = U256::zero();
    let mut expected_channel = None;
    t.execute_with(|| {
        create_channel(chain_id, channel_id, Default::default());
        assert_ok!(Messenger::do_open_channel(chain_id, channel_id));
        expected_channel = Channels::<Runtime>::get(chain_id, channel_id);
    });

    let (state_root, storage_key, storage_proof) =
        crate::mock::storage_proof_of_channels::<Runtime>(t.as_backend(), chain_id, channel_id);
    let res: Result<Channel<Balance>, VerificationError> =
        StorageProofVerifier::<Blake2Hasher>::get_decoded_value(
            &state_root,
            storage_proof,
            storage_key,
        );

    assert!(res.is_ok());
    assert_eq!(res.unwrap(), expected_channel.unwrap())
}

fn open_channel_between_chains(
    chain_a_test_ext: &mut TestExternalities,
    chain_b_test_ext: &mut TestExternalities,
    fee_model: FeeModel<Balance>,
) -> ChannelId {
    let chain_a_id = chain_a::SelfChainId::get();
    let chain_b_id = chain_b::SelfChainId::get();

    // initiate channel open on chain_a
    let channel_id = chain_a_test_ext.execute_with(|| -> ChannelId {
        let channel_id = U256::zero();
        create_channel(chain_b_id, channel_id, fee_model);
        channel_id
    });

    channel_relay_request_and_response(
        chain_a_test_ext,
        chain_b_test_ext,
        channel_id,
        Nonce::zero(),
        true,
        MessageWeightTag::ProtocolChannelOpen,
        None,
    );

    // check channel state be open on chain_b
    chain_b_test_ext.execute_with(|| {
        let channel = chain_b::Messenger::channels(chain_a_id, channel_id).unwrap();
        assert_eq!(channel.state, ChannelState::Open);
        chain_b::System::assert_has_event(chain_b::RuntimeEvent::Messenger(crate::Event::<
            chain_b::Runtime,
        >::ChannelInitiated {
            chain_id: chain_a_id,
            channel_id,
        }));
        chain_b::System::assert_has_event(chain_b::RuntimeEvent::Messenger(crate::Event::<
            chain_b::Runtime,
        >::ChannelOpen {
            chain_id: chain_a_id,
            channel_id,
        }));

        // check inbox response storage key generation
        let messages_with_keys = chain_b::Messenger::get_block_messages();
        assert_eq!(messages_with_keys.outbox.len(), 0);
        assert_eq!(messages_with_keys.inbox_responses.len(), 1);
        let expected_key = InboxResponses::<chain_b::Runtime>::hashed_key_for((
            chain_a_id,
            channel_id,
            Nonce::zero(),
        ));
        assert_eq!(
            messages_with_keys.inbox_responses[0].storage_key,
            expected_key
        );
    });

    // check channel state be open on chain_a
    chain_a_test_ext.execute_with(|| {
        let channel = chain_a::Messenger::channels(chain_b_id, channel_id).unwrap();
        assert_eq!(channel.state, ChannelState::Open);
        assert_eq!(
            channel.latest_response_received_message_nonce,
            Some(Nonce::zero())
        );
        assert_eq!(channel.next_inbox_nonce, Nonce::zero());
        assert_eq!(channel.next_outbox_nonce, Nonce::one());
        chain_a::System::assert_has_event(chain_a::RuntimeEvent::Messenger(crate::Event::<
            chain_a::Runtime,
        >::ChannelOpen {
            chain_id: chain_b_id,
            channel_id,
        }));
    });

    channel_id
}

fn send_message_between_chains(
    sender: &AccountId,
    chain_a_test_ext: &mut TestExternalities,
    chain_b_test_ext: &mut TestExternalities,
    msg: EndpointPayload,
    channel_id: ChannelId,
) {
    let chain_b_id = chain_b::SelfChainId::get();

    // send message form outbox
    chain_a_test_ext.execute_with(|| {
        let resp = <chain_a::Messenger as Sender<AccountId>>::send_message(
            sender,
            chain_b_id,
            EndpointRequest {
                src_endpoint: Endpoint::Id(0),
                dst_endpoint: Endpoint::Id(0),
                payload: msg,
            },
        );
        assert_ok!(resp);
    });

    channel_relay_request_and_response(
        chain_a_test_ext,
        chain_b_test_ext,
        channel_id,
        Nonce::one(),
        false,
        Default::default(),
        Some(Endpoint::Id(0)),
    );

    // check state on chain_b
    chain_b_test_ext.execute_with(|| {
        // Outbox, Outbox responses, Inbox, InboxResponses must be empty
        assert_eq!(Outbox::<chain_b::Runtime>::count(), 0);
        assert!(OutboxResponses::<chain_b::Runtime>::get().is_none());
        assert!(Inbox::<chain_b::Runtime>::get().is_none());

        // latest inbox message response is cleared on next message
        assert_eq!(InboxResponses::<chain_b::Runtime>::count(), 1);
    });

    // check state on chain_a
    chain_a_test_ext.execute_with(|| {
        // Outbox, Outbox responses, Inbox, InboxResponses must be empty
        assert_eq!(Outbox::<chain_a::Runtime>::count(), 0);
        assert!(OutboxResponses::<chain_a::Runtime>::get().is_none());
        assert!(Inbox::<chain_a::Runtime>::get().is_none());
        assert_eq!(InboxResponses::<chain_a::Runtime>::count(), 0);

        let channel = chain_a::Messenger::channels(chain_b_id, channel_id).unwrap();
        assert_eq!(
            channel.latest_response_received_message_nonce,
            Some(Nonce::one())
        );
    });
}

fn close_channel_between_chains(
    chain_a_test_ext: &mut TestExternalities,
    chain_b_test_ext: &mut TestExternalities,
    channel_id: ChannelId,
) {
    let chain_a_id = chain_a::SelfChainId::get();
    let chain_b_id = chain_b::SelfChainId::get();

    // initiate channel close on chain_a
    chain_a_test_ext.execute_with(|| {
        close_channel(chain_b_id, channel_id, Some(Nonce::zero()));
    });

    channel_relay_request_and_response(
        chain_a_test_ext,
        chain_b_test_ext,
        channel_id,
        Nonce::one(),
        true,
        MessageWeightTag::ProtocolChannelClose,
        None,
    );

    // check channel state be close on chain_b
    chain_b_test_ext.execute_with(|| {
        let channel = chain_b::Messenger::channels(chain_a_id, channel_id).unwrap();
        assert_eq!(channel.state, ChannelState::Closed);
        chain_b::System::assert_has_event(chain_b::RuntimeEvent::Messenger(crate::Event::<
            chain_b::Runtime,
        >::ChannelClosed {
            chain_id: chain_a_id,
            channel_id,
        }));

        assert_eq!(channel.latest_response_received_message_nonce, None);
        assert_eq!(
            channel.next_inbox_nonce,
            Nonce::one().checked_add(Nonce::one()).unwrap()
        );
        assert_eq!(channel.next_outbox_nonce, Nonce::zero());

        // Outbox, Outbox responses, Inbox, InboxResponses must be empty
        assert_eq!(Outbox::<chain_b::Runtime>::count(), 0);
        assert!(OutboxResponses::<chain_b::Runtime>::get().is_none());
        assert!(Inbox::<chain_b::Runtime>::get().is_none());

        // latest inbox message response is cleared on next message
        assert_eq!(InboxResponses::<chain_b::Runtime>::count(), 1);
    });

    // check channel state be closed on chain_a
    chain_a_test_ext.execute_with(|| {
        let channel = chain_a::Messenger::channels(chain_b_id, channel_id).unwrap();
        assert_eq!(channel.state, ChannelState::Closed);
        assert_eq!(
            channel.latest_response_received_message_nonce,
            Some(Nonce::one())
        );
        assert_eq!(channel.next_inbox_nonce, Nonce::zero());
        assert_eq!(
            channel.next_outbox_nonce,
            Nonce::one().checked_add(Nonce::one()).unwrap()
        );
        chain_a::System::assert_has_event(chain_a::RuntimeEvent::Messenger(crate::Event::<
            chain_a::Runtime,
        >::ChannelClosed {
            chain_id: chain_b_id,
            channel_id,
        }));

        // Outbox, Outbox responses, Inbox, InboxResponses must be empty
        assert_eq!(Outbox::<chain_a::Runtime>::count(), 0);
        assert!(OutboxResponses::<chain_a::Runtime>::get().is_none());
        assert!(Inbox::<chain_a::Runtime>::get().is_none());
        assert_eq!(InboxResponses::<chain_a::Runtime>::count(), 0);
    })
}

fn force_toggle_channel_state<Runtime: crate::Config>(
    dst_chain_id: ChainId,
    channel_id: ChannelId,
    toggle: bool,
) {
    let fee_model = FeeModel {
        relay_fee: Default::default(),
    };
    let init_params = InitiateChannelParams {
        max_outgoing_messages: 100,
        fee_model,
    };

    let channel = Pallet::<Runtime>::channels(dst_chain_id, channel_id).unwrap_or_else(|| {
        Pallet::<Runtime>::do_init_channel(dst_chain_id, init_params).unwrap();
        Pallet::<Runtime>::channels(dst_chain_id, channel_id).unwrap()
    });

    if !toggle {
        return;
    }

    if channel.state == ChannelState::Initiated {
        Pallet::<Runtime>::do_open_channel(dst_chain_id, channel_id).unwrap();
    }

    if channel.state == ChannelState::Open {
        Pallet::<Runtime>::do_close_channel(dst_chain_id, channel_id).unwrap();
    }
}

fn channel_relay_request_and_response(
    chain_a_test_ext: &mut TestExternalities,
    chain_b_test_ext: &mut TestExternalities,
    channel_id: ChannelId,
    nonce: Nonce,
    toggle_channel_state: bool,
    weight_tag: MessageWeightTag,
    maybe_endpoint: Option<Endpoint>,
) {
    let chain_a_id = chain_a::SelfChainId::get();
    let chain_b_id = chain_b::SelfChainId::get();

    // relay message to chain_b
    let msg = chain_a_test_ext
        .execute_with(|| Outbox::<chain_a::Runtime>::get((chain_b_id, channel_id, nonce)).unwrap());
    let (_state_root, _key, message_proof) = storage_proof_of_outbox_messages::<chain_a::Runtime>(
        chain_a_test_ext.as_backend(),
        chain_b_id,
        channel_id,
        nonce,
    );

    let xdm = CrossDomainMessage {
        src_chain_id: chain_a_id,
        dst_chain_id: chain_b_id,
        channel_id,
        nonce,
        proof: Proof::Domain {
            consensus_chain_mmr_proof: default_consensus_proof(),
            domain_proof: StorageProof::empty(),
            message_proof,
        },
        weight_tag: maybe_endpoint
            .clone()
            .map(MessageWeightTag::EndpointRequest)
            .unwrap_or(weight_tag.clone()),
    };
    chain_b_test_ext.execute_with(|| {
        force_toggle_channel_state::<chain_b::Runtime>(
            chain_a_id,
            channel_id,
            toggle_channel_state,
        );
        Inbox::<chain_b::Runtime>::set(Some(msg));

        // process inbox message
        let result = chain_b::Messenger::relay_message(chain_b::RuntimeOrigin::none(), xdm);
        assert_ok!(result);

        chain_b::System::assert_has_event(chain_b::RuntimeEvent::Messenger(crate::Event::<
            chain_b::Runtime,
        >::InboxMessageResponse {
            chain_id: chain_a_id,
            channel_id,
            nonce,
        }));

        let response =
            chain_b::Messenger::inbox_responses((chain_a_id, channel_id, nonce)).unwrap();
        assert_eq!(response.src_chain_id, chain_b_id);
        assert_eq!(response.dst_chain_id, chain_a_id);
        assert_eq!(response.channel_id, channel_id);
        assert_eq!(response.nonce, nonce);
        assert_eq!(chain_a::Messenger::inbox(), None);
    });

    // relay message response to chain_a
    let (_state_root, _key, message_proof) =
        storage_proof_of_inbox_message_responses::<chain_b::Runtime>(
            chain_b_test_ext.as_backend(),
            chain_a_id,
            channel_id,
            nonce,
        );

    let msg = chain_b_test_ext.execute_with(|| {
        InboxResponses::<chain_b::Runtime>::get((chain_a_id, channel_id, nonce)).unwrap()
    });

    let xdm = CrossDomainMessage {
        src_chain_id: chain_b_id,
        dst_chain_id: chain_a_id,
        channel_id,
        nonce,
        proof: Proof::Consensus {
            consensus_chain_mmr_proof: default_consensus_proof(),
            message_proof,
        },
        weight_tag: maybe_endpoint
            .clone()
            .map(MessageWeightTag::EndpointResponse)
            .unwrap_or(weight_tag),
    };
    chain_a_test_ext.execute_with(|| {
        force_toggle_channel_state::<chain_a::Runtime>(
            chain_b_id,
            channel_id,
            toggle_channel_state,
        );
        OutboxResponses::<chain_a::Runtime>::set(Some(msg));

        // process outbox message response
        let result =
            chain_a::Messenger::relay_message_response(chain_a::RuntimeOrigin::none(), xdm);
        assert_ok!(result);

        // outbox message and message response should not exists
        assert_eq!(
            chain_a::Messenger::outbox((chain_b_id, channel_id, nonce)),
            None
        );
        assert_eq!(chain_a::Messenger::outbox_responses(), None);

        chain_a::System::assert_has_event(chain_a::RuntimeEvent::Messenger(crate::Event::<
            chain_a::Runtime,
        >::OutboxMessageResult {
            chain_id: chain_b_id,
            channel_id,
            nonce,
            result: OutboxMessageResult::Ok,
        }));
    })
}

#[test]
fn test_open_channel_between_chains() {
    let mut chain_a_test_ext = chain_a::new_test_ext();
    let mut chain_b_test_ext = chain_b::new_test_ext();
    // open channel between chain_a and chain_b
    // chain_a initiates the channel open
    open_channel_between_chains(
        &mut chain_a_test_ext,
        &mut chain_b_test_ext,
        Default::default(),
    );
}

#[test]
fn test_close_channel_between_chains() {
    let mut chain_a_test_ext = chain_a::new_test_ext();
    let mut chain_b_test_ext = chain_b::new_test_ext();
    // open channel between chain_a and chain_b
    // chain_a initiates the channel open
    let channel_id = open_channel_between_chains(
        &mut chain_a_test_ext,
        &mut chain_b_test_ext,
        Default::default(),
    );

    // close open channel
    close_channel_between_chains(&mut chain_a_test_ext, &mut chain_b_test_ext, channel_id)
}

#[test]
fn test_send_message_between_chains() {
    let mut chain_a_test_ext = chain_a::new_test_ext();
    let mut chain_b_test_ext = chain_b::new_test_ext();
    // open channel between chain_a and chain_b
    // chain_a initiates the channel open
    let channel_id = open_channel_between_chains(
        &mut chain_a_test_ext,
        &mut chain_b_test_ext,
        Default::default(),
    );

    // send message
    send_message_between_chains(
        &0,
        &mut chain_a_test_ext,
        &mut chain_b_test_ext,
        vec![1, 2, 3, 4],
        channel_id,
    )
}

fn initiate_transfer_on_chain(chain_a_ext: &mut TestExternalities) {
    // this account should have 1000 balance on each chain
    let account_id = 1;
    chain_a_ext.execute_with(|| {
        let res = chain_a::Transporter::transfer(
            chain_a::RuntimeOrigin::signed(account_id),
            Location {
                chain_id: chain_b::SelfChainId::get(),
                account_id: chain_b::MockAccountIdConverter::convert(account_id),
            },
            500,
        );
        assert_ok!(res);
        chain_a::System::assert_has_event(chain_a::RuntimeEvent::Transporter(
            pallet_transporter::Event::<chain_a::Runtime>::OutgoingTransferInitiated {
                chain_id: chain_b::SelfChainId::get(),
                message_id: (U256::zero(), U256::one()),
            },
        ));
        chain_a::System::assert_has_event(chain_a::RuntimeEvent::Messenger(crate::Event::<
            chain_a::Runtime,
        >::OutboxMessage {
            chain_id: chain_b::SelfChainId::get(),
            channel_id: U256::zero(),
            nonce: U256::one(),
        }));
        assert!(chain_a::Transporter::outgoing_transfers(
            chain_b::SelfChainId::get(),
            (U256::zero(), U256::one()),
        )
        .is_some())
    })
}

fn verify_transfer_on_chain(
    chain_a_ext: &mut TestExternalities,
    chain_b_ext: &mut TestExternalities,
) {
    // this account should have 496 balance with 1 fee left
    // chain a should have
    //   a successful event
    //   reduced balance
    //   empty state
    let account_id = 1;
    chain_a_ext.execute_with(|| {
        chain_a::System::assert_has_event(chain_a::RuntimeEvent::Transporter(
            pallet_transporter::Event::<chain_a::Runtime>::OutgoingTransferSuccessful {
                chain_id: chain_b::SelfChainId::get(),
                message_id: (U256::zero(), U256::one()),
            },
        ));
        assert!(chain_a::Transporter::outgoing_transfers(
            chain_b::SelfChainId::get(),
            (U256::zero(), U256::one()),
        )
        .is_none())
    });

    // chain a should have
    //   a successful event incoming event
    //   increased balance
    chain_b_ext.execute_with(|| {
        chain_b::System::assert_has_event(chain_b::RuntimeEvent::Transporter(
            pallet_transporter::Event::<chain_b::Runtime>::IncomingTransferSuccessful {
                chain_id: chain_a::SelfChainId::get(),
                message_id: (U256::zero(), U256::one()),
            },
        ));
        chain_b::System::assert_has_event(chain_b::RuntimeEvent::Messenger(crate::Event::<
            chain_b::Runtime,
        >::InboxMessageResponse {
            chain_id: chain_a::SelfChainId::get(),
            channel_id: U256::zero(),
            nonce: U256::one(),
        }));
        assert_eq!(chain_b::Balances::free_balance(account_id), 500000500);
    })
}

#[test]
fn test_transport_funds_between_chains() {
    let mut chain_a_test_ext = chain_a::new_test_ext();
    let mut chain_b_test_ext = chain_b::new_test_ext();

    // open channel between chain_a and chain_b
    // chain_a initiates the channel open
    let channel_id = open_channel_between_chains(
        &mut chain_a_test_ext,
        &mut chain_b_test_ext,
        FeeModel { relay_fee: 1 },
    );

    // initiate transfer
    initiate_transfer_on_chain(&mut chain_a_test_ext);

    // relay message
    channel_relay_request_and_response(
        &mut chain_a_test_ext,
        &mut chain_b_test_ext,
        channel_id,
        Nonce::one(),
        false,
        Default::default(),
        Some(Endpoint::Id(100)),
    );

    // post check
    verify_transfer_on_chain(&mut chain_a_test_ext, &mut chain_b_test_ext)
}

#[test]
fn test_transport_funds_between_chains_failed_low_balance() {
    let mut chain_a_test_ext = chain_a::new_test_ext();
    let mut chain_b_test_ext = chain_b::new_test_ext();
    // open channel between chain_a and chain_b
    // chain_a initiates the channel open
    open_channel_between_chains(
        &mut chain_a_test_ext,
        &mut chain_b_test_ext,
        Default::default(),
    );

    // initiate transfer
    let account_id = 100;
    chain_a_test_ext.execute_with(|| {
        let res = chain_a::Transporter::transfer(
            chain_a::RuntimeOrigin::signed(account_id),
            Location {
                chain_id: chain_b::SelfChainId::get(),
                account_id: chain_b::MockAccountIdConverter::convert(account_id),
            },
            500,
        );
        assert_err!(
            res,
            pallet_transporter::Error::<chain_a::Runtime>::LowBalance
        );
    });
}

#[test]
fn test_transport_funds_between_chains_failed_no_open_channel() {
    let mut chain_a_test_ext = chain_a::new_test_ext();

    // initiate transfer
    let account_id = 1;
    chain_a_test_ext.execute_with(|| {
        let res = chain_a::Transporter::transfer(
            chain_a::RuntimeOrigin::signed(account_id),
            Location {
                chain_id: chain_b::SelfChainId::get(),
                account_id: chain_b::MockAccountIdConverter::convert(account_id),
            },
            500,
        );
        assert_err!(res, crate::Error::<chain_a::Runtime>::NoOpenChannel);
    });
}
