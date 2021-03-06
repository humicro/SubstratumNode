// Copyright (c) 2017-2019, Substratum LLC (https://substratum.net) and/or its affiliates. All rights reserved.
use crate::sub_lib::cryptde::PublicKey;
use crate::sub_lib::dispatcher::Component;
use crate::sub_lib::hopper::ExpiredCoresPackage;
use crate::sub_lib::node_addr::NodeAddr;
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::route::Route;
use crate::sub_lib::stream_handler_pool::DispatcherNodeQueryResponse;
use crate::sub_lib::stream_handler_pool::TransmitDataMsg;
use crate::sub_lib::wallet::Wallet;
use actix::Message;
use actix::Recipient;
use actix::Syn;
use std::net::IpAddr;
use std::net::Ipv4Addr;

pub const SENTINEL_IP_OCTETS: [u8; 4] = [255, 255, 255, 255];

pub fn sentinel_ip_addr() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(
        SENTINEL_IP_OCTETS[0],
        SENTINEL_IP_OCTETS[1],
        SENTINEL_IP_OCTETS[2],
        SENTINEL_IP_OCTETS[3],
    ))
}

#[derive(Clone, PartialEq, Debug)]
pub struct NeighborhoodConfig {
    pub neighbor_configs: Vec<(PublicKey, NodeAddr)>,
    pub is_bootstrap_node: bool,
    pub local_ip_addr: IpAddr,
    pub clandestine_port_list: Vec<u16>,
    pub earning_wallet: Wallet,
    pub consuming_wallet: Option<Wallet>,
}

impl NeighborhoodConfig {
    pub fn is_decentralized(&self) -> bool {
        !self.neighbor_configs.is_empty()
            && (self.local_ip_addr != sentinel_ip_addr())
            && !self.clandestine_port_list.is_empty()
    }
}

#[derive(Clone)]
pub struct NeighborhoodSubs {
    pub bind: Recipient<Syn, BindMessage>,
    pub bootstrap: Recipient<Syn, BootstrapNeighborhoodNowMessage>,
    pub node_query: Recipient<Syn, NodeQueryMessage>,
    pub route_query: Recipient<Syn, RouteQueryMessage>,
    pub from_hopper: Recipient<Syn, ExpiredCoresPackage>,
    pub dispatcher_node_query: Recipient<Syn, DispatcherNodeQueryMessage>,
    pub remove_neighbor: Recipient<Syn, RemoveNeighborMessage>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeDescriptor {
    pub public_key: PublicKey,
    pub node_addr_opt: Option<NodeAddr>,
}

impl NodeDescriptor {
    pub fn new(public_key: PublicKey, node_addr_opt: Option<NodeAddr>) -> NodeDescriptor {
        NodeDescriptor {
            public_key,
            node_addr_opt,
        }
    }
}

#[derive(Message, Clone)]
pub struct BootstrapNeighborhoodNowMessage {}

#[derive(Debug, PartialEq, Clone)]
pub enum NodeQueryMessage {
    IpAddress(IpAddr),
    PublicKey(PublicKey),
}

impl Message for NodeQueryMessage {
    type Result = Option<NodeDescriptor>;
}

#[derive(Message, Clone)]
pub struct DispatcherNodeQueryMessage {
    pub query: NodeQueryMessage,
    pub context: TransmitDataMsg,
    pub recipient: Recipient<Syn, DispatcherNodeQueryResponse>,
}

#[derive(PartialEq, Clone, Debug, Copy)]
pub enum TargetType {
    Bootstrap,
    Standard,
}

#[derive(PartialEq, Debug)]
pub struct RouteQueryMessage {
    pub target_type: TargetType,
    pub target_key_opt: Option<PublicKey>,
    pub target_component: Component,
    pub minimum_hop_count: usize,
    pub return_component_opt: Option<Component>,
}

impl Message for RouteQueryMessage {
    type Result = Option<RouteQueryResponse>;
}

impl RouteQueryMessage {
    pub fn data_indefinite_route_request(minimum_hop_count: usize) -> RouteQueryMessage {
        RouteQueryMessage {
            target_type: TargetType::Standard,
            target_key_opt: None,
            target_component: Component::ProxyClient,
            minimum_hop_count,
            return_component_opt: Some(Component::ProxyServer),
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ExpectedService {
    Routing(PublicKey, Wallet),
    Exit(PublicKey, Wallet),
    Nothing,
}

#[derive(PartialEq, Debug, Clone)]
pub enum ExpectedServices {
    OneWay(Vec<ExpectedService>),
    RoundTrip(Vec<ExpectedService>, Vec<ExpectedService>, u32),
}

#[derive(PartialEq, Debug, Clone)]
pub struct RouteQueryResponse {
    pub route: Route,
    pub expected_services: ExpectedServices,
}

#[derive(PartialEq, Debug, Message, Clone)]
pub struct RemoveNeighborMessage {
    pub public_key: PublicKey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn data_indefinite_route_request() {
        let result = RouteQueryMessage::data_indefinite_route_request(2);

        assert_eq!(
            result,
            RouteQueryMessage {
                target_type: TargetType::Standard,
                target_key_opt: None,
                target_component: Component::ProxyClient,
                minimum_hop_count: 2,
                return_component_opt: Some(Component::ProxyServer),
            }
        );
    }

    #[test]
    fn neighborhood_config_is_not_decentralized_if_there_are_no_neighbor_configs() {
        let subject = NeighborhoodConfig {
            neighbor_configs: vec![],
            earning_wallet: Wallet::new("router"),
            consuming_wallet: Some(Wallet::new("consumer")),
            is_bootstrap_node: false,
            local_ip_addr: IpAddr::from_str("1.2.3.4").unwrap(),
            clandestine_port_list: vec![1234],
        };

        let result = subject.is_decentralized();

        assert_eq!(result, false);
    }

    #[test]
    fn neighborhood_config_is_not_decentralized_if_the_sentinel_ip_address_is_used() {
        let subject = NeighborhoodConfig {
            neighbor_configs: vec![(
                PublicKey::new(&b"key"[..]),
                NodeAddr::new(&IpAddr::from_str("2.3.4.5").unwrap(), &vec![2345]),
            )],
            earning_wallet: Wallet::new("router"),
            consuming_wallet: Some(Wallet::new("consumer")),
            is_bootstrap_node: false,
            local_ip_addr: sentinel_ip_addr(),
            clandestine_port_list: vec![1234],
        };

        let result = subject.is_decentralized();

        assert_eq!(result, false);
    }

    #[test]
    fn neighborhood_config_is_not_decentralized_if_there_are_no_clandestine_ports() {
        let subject = NeighborhoodConfig {
            neighbor_configs: vec![(
                PublicKey::new(&b"key"[..]),
                NodeAddr::new(&IpAddr::from_str("2.3.4.5").unwrap(), &vec![2345]),
            )],
            earning_wallet: Wallet::new("router"),
            consuming_wallet: Some(Wallet::new("consumer")),
            is_bootstrap_node: false,
            local_ip_addr: IpAddr::from_str("1.2.3.4").unwrap(),
            clandestine_port_list: vec![],
        };

        let result = subject.is_decentralized();

        assert_eq!(result, false);
    }

    #[test]
    fn neighborhood_config_is_decentralized_if_neighbor_config_and_local_ip_addr_and_clandestine_port(
    ) {
        let subject = NeighborhoodConfig {
            neighbor_configs: vec![(
                PublicKey::new(&b"key"[..]),
                NodeAddr::new(&IpAddr::from_str("2.3.4.5").unwrap(), &vec![2345]),
            )],
            earning_wallet: Wallet::new("router"),
            consuming_wallet: Some(Wallet::new("consumer")),
            is_bootstrap_node: false,
            local_ip_addr: IpAddr::from_str("1.2.3.4").unwrap(),
            clandestine_port_list: vec![1234],
        };

        let result = subject.is_decentralized();

        assert_eq!(result, true);
    }
}
