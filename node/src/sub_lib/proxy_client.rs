// Copyright (c) 2017-2019, Substratum LLC (https://substratum.net) and/or its affiliates. All rights reserved.
use crate::sub_lib::hopper::ExpiredCoresPackage;
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::sequence_buffer::SequencedPacket;
use crate::sub_lib::stream_key::StreamKey;
use actix::Message;
use actix::Recipient;
use actix::Syn;
use serde_derive::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;

pub const TEMPORARY_PER_EXIT_BYTE_RATE: u64 = 2;
pub const TEMPORARY_PER_EXIT_RATE: u64 = 1;

pub fn error_socket_addr() -> SocketAddr {
    SocketAddr::from(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0))
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ClientResponsePayload {
    pub stream_key: StreamKey,
    pub sequenced_packet: SequencedPacket,
}

#[derive(Clone)]
pub struct ProxyClientSubs {
    pub bind: Recipient<Syn, BindMessage>,
    pub from_hopper: Recipient<Syn, ExpiredCoresPackage>,
    pub inbound_server_data: Recipient<Syn, InboundServerData>,
}

impl ClientResponsePayload {
    pub fn make_terminating_payload(stream_key: StreamKey) -> ClientResponsePayload {
        ClientResponsePayload {
            stream_key,
            sequenced_packet: SequencedPacket {
                data: vec![],
                sequence_number: 0,
                last_data: true,
            },
        }
    }
}

#[derive(PartialEq, Clone, Message, Debug)]
pub struct InboundServerData {
    pub stream_key: StreamKey,
    pub last_data: bool,
    pub sequence_number: u64,
    pub source: SocketAddr,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_lib::utils::tests::make_meaningless_stream_key;

    #[test]
    fn make_terminating_payload_makes_terminating_payload() {
        let stream_key: StreamKey = make_meaningless_stream_key();

        let payload = ClientResponsePayload::make_terminating_payload(stream_key);

        assert_eq!(
            payload,
            ClientResponsePayload {
                stream_key,
                sequenced_packet: SequencedPacket {
                    data: vec!(),
                    sequence_number: 0,
                    last_data: true
                },
            }
        )
    }
}
