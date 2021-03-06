// Copyright (c) 2017-2019, Substratum LLC (https://substratum.net) and/or its affiliates. All rights reserved.
use crate::substratum_node::SubstratumNode;
use node_lib::neighborhood::gossip::Gossip;
use node_lib::neighborhood::gossip::GossipNodeRecord;
use node_lib::neighborhood::neighborhood_database::NodeRecordInner;
use node_lib::neighborhood::neighborhood_database::NodeSignatures;
use node_lib::sub_lib::cryptde::CryptDE;
use node_lib::sub_lib::cryptde::PublicKey;
use node_lib::sub_lib::cryptde_null::CryptDENull;
use node_lib::sub_lib::dispatcher::Component;
use node_lib::sub_lib::hopper::IncipientCoresPackage;
use node_lib::sub_lib::route::Route;
use node_lib::sub_lib::route::RouteSegment;
use node_lib::sub_lib::wallet::Wallet;

pub struct GossipBuilder {
    consuming_wallet: Option<Wallet>,
    node_info: Vec<GossipBuilderNodeInfo>,
    connection_pairs: Vec<(PublicKey, PublicKey)>,
}

impl GossipBuilder {
    pub fn new(consuming_wallet: Option<Wallet>) -> GossipBuilder {
        GossipBuilder {
            consuming_wallet,
            node_info: vec![],
            connection_pairs: vec![],
        }
    }

    pub fn add_node(
        mut self,
        node: &dyn SubstratumNode,
        is_bootstrap: bool,
        include_ip: bool,
    ) -> Self {
        self.node_info.push(GossipBuilderNodeInfo {
            node_record_inner: NodeRecordInner {
                public_key: node.public_key(),
                node_addr_opt: match include_ip {
                    true => Some(node.node_addr()),
                    false => None,
                },
                is_bootstrap_node: is_bootstrap,
                earning_wallet: node.earning_wallet().clone(),
                consuming_wallet: node.consuming_wallet().clone(),
                neighbors: vec![],
                version: 0,
            },
            cryptde: Box::new(CryptDENull::from(&node.public_key())),
        });
        self
    }

    pub fn add_gnr(mut self, gnr: &GossipNodeRecord) -> Self {
        self.node_info.push(GossipBuilderNodeInfo {
            node_record_inner: NodeRecordInner {
                public_key: gnr.public_key(),
                node_addr_opt: gnr.inner.node_addr_opt.clone(),
                is_bootstrap_node: gnr.inner.is_bootstrap_node,
                earning_wallet: gnr.inner.earning_wallet.clone(),
                consuming_wallet: gnr.inner.consuming_wallet.clone(),
                neighbors: vec![],
                version: gnr.inner.version,
            },
            cryptde: Box::new(CryptDENull::from(&gnr.public_key())),
        });
        self
    }

    pub fn add_fictional_node(mut self, node_record: NodeRecordInner) -> Self {
        let key = node_record.public_key.clone();
        self.node_info.push(GossipBuilderNodeInfo {
            node_record_inner: node_record,
            cryptde: Box::new(CryptDENull::from(&key)),
        });
        self
    }

    pub fn add_connection(mut self, from_key: &PublicKey, to_key: &PublicKey) -> Self {
        self.connection_pairs
            .push((from_key.clone(), to_key.clone()));
        self
    }

    pub fn build(self) -> Gossip {
        let mut node_records: Vec<GossipNodeRecord> = self
            .node_info
            .into_iter()
            .map(|node_info| {
                let signatures =
                    NodeSignatures::from(node_info.cryptde.as_ref(), &node_info.node_record_inner);
                GossipNodeRecord {
                    inner: node_info.node_record_inner,
                    signatures,
                }
            })
            .collect();

        self.connection_pairs.iter ().for_each (|pair_ref| {
            let from_key = pair_ref.0.clone ();
            let from_node_ref_opt = node_records.iter_mut ().find (|n| n.inner.public_key == from_key);
            let to_key = pair_ref.1.clone ();
            if let Some (from_node_ref) = from_node_ref_opt {
                from_node_ref.inner.neighbors.push (to_key);
            }
            else {
                panic! ("You directed that {:?} should be made a neighbor of {:?}, but {:?} was never added to the GossipBuilder",
                    to_key, from_key, from_key)
            }
        });
        Gossip { node_records }
    }

    pub fn build_cores_package(self, from: &PublicKey, to: &PublicKey) -> IncipientCoresPackage {
        let consuming_wallet = self.consuming_wallet.clone();
        let gossip = self.build();
        IncipientCoresPackage::new(
            &CryptDENull::new(),
            Route::one_way(
                RouteSegment::new(vec![from, to], Component::Neighborhood),
                &CryptDENull::from(from),
                consuming_wallet,
            )
            .unwrap(),
            gossip,
            to,
        )
        .unwrap()
    }
}

struct GossipBuilderNodeInfo {
    node_record_inner: NodeRecordInner,
    cryptde: Box<dyn CryptDE>,
}
