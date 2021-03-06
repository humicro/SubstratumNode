// Copyright (c) 2017-2019, Substratum LLC (https://substratum.net) and/or its affiliates. All rights reserved.
use crate::proxy_client::resolver_wrapper::ResolverWrapperFactory;
use crate::proxy_client::resolver_wrapper::ResolverWrapperFactoryReal;
use crate::proxy_client::stream_handler_pool::StreamHandlerPool;
use crate::proxy_client::stream_handler_pool::StreamHandlerPoolFactory;
use crate::proxy_client::stream_handler_pool::StreamHandlerPoolFactoryReal;
use crate::sub_lib::accountant::ReportExitServiceProvidedMessage;
use crate::sub_lib::cryptde::CryptDE;
use crate::sub_lib::cryptde::PublicKey;
use crate::sub_lib::hopper::ExpiredCoresPackage;
use crate::sub_lib::hopper::IncipientCoresPackage;
use crate::sub_lib::logger::Logger;
use crate::sub_lib::peer_actors::BindMessage;
use crate::sub_lib::proxy_client::ClientResponsePayload;
use crate::sub_lib::proxy_client::InboundServerData;
use crate::sub_lib::proxy_client::ProxyClientSubs;
use crate::sub_lib::proxy_client::TEMPORARY_PER_EXIT_BYTE_RATE;
use crate::sub_lib::proxy_client::TEMPORARY_PER_EXIT_RATE;
use crate::sub_lib::proxy_server::ClientRequestPayload;
use crate::sub_lib::route::Route;
use crate::sub_lib::sequence_buffer::SequencedPacket;
use crate::sub_lib::stream_key::StreamKey;
use crate::sub_lib::utils::NODE_MAILBOX_CAPACITY;
use crate::sub_lib::wallet::Wallet;
use actix::Actor;
use actix::Addr;
use actix::Context;
use actix::Handler;
use actix::Recipient;
use actix::Syn;
use std::collections::HashMap;
use std::net::SocketAddr;
use trust_dns_resolver::config::NameServerConfig;
use trust_dns_resolver::config::Protocol;
use trust_dns_resolver::config::ResolverConfig;
use trust_dns_resolver::config::ResolverOpts;

pub struct ProxyClient {
    dns_servers: Vec<SocketAddr>,
    resolver_wrapper_factory: Box<dyn ResolverWrapperFactory>,
    stream_handler_pool_factory: Box<dyn StreamHandlerPoolFactory>,
    cryptde: &'static dyn CryptDE,
    to_hopper: Option<Recipient<Syn, IncipientCoresPackage>>,
    to_accountant: Option<Recipient<Syn, ReportExitServiceProvidedMessage>>,
    pool: Option<Box<dyn StreamHandlerPool>>,
    stream_contexts: HashMap<StreamKey, StreamContext>,
    logger: Logger,
}

impl Actor for ProxyClient {
    type Context = Context<Self>;
}

impl Handler<BindMessage> for ProxyClient {
    type Result = ();

    fn handle(&mut self, msg: BindMessage, ctx: &mut Self::Context) -> Self::Result {
        self.logger.debug(format!("Handling BindMessage"));
        ctx.set_mailbox_capacity(NODE_MAILBOX_CAPACITY);
        self.to_hopper = Some(msg.peer_actors.hopper.from_hopper_client);
        self.to_accountant = Some(msg.peer_actors.accountant.report_exit_service_provided);
        let mut config = ResolverConfig::new();
        for dns_server_ref in &self.dns_servers {
            self.logger
                .info(format!("Adding DNS server: {}", dns_server_ref.ip()));
            config.add_name_server(NameServerConfig {
                socket_addr: *dns_server_ref,
                protocol: Protocol::Udp,
                tls_dns_name: None,
            })
        }
        let opts = ResolverOpts::default();
        let resolver = self.resolver_wrapper_factory.make(config, opts);
        self.pool = Some(self.stream_handler_pool_factory.make(
            resolver,
            self.cryptde,
            self.to_accountant.clone().expect("Accountant is unbound"),
            msg.peer_actors.proxy_client.inbound_server_data,
        ));
        ()
    }
}

impl Handler<ExpiredCoresPackage> for ProxyClient {
    type Result = ();

    fn handle(&mut self, msg: ExpiredCoresPackage, _ctx: &mut Self::Context) -> Self::Result {
        let payload = match msg.payload::<ClientRequestPayload>(self.cryptde) {
            Ok(payload) => payload,
            Err(e) => {
                self.logger.error(format!(
                    "Error ('{}') interpreting payload for transmission: {:?}",
                    e,
                    msg.payload_data().as_slice()
                ));
                return ();
            }
        };
        let pool = self.pool.as_mut().expect("StreamHandlerPool unbound");
        let consuming_wallet = msg.consuming_wallet;
        let return_route = msg.remaining_route;
        let latest_stream_context = StreamContext {
            return_route,
            payload_destination_key: payload.originator_public_key.clone(),
            consuming_wallet: consuming_wallet.clone(),
        };
        self.stream_contexts
            .insert(payload.stream_key.clone(), latest_stream_context);
        pool.process_package(payload, consuming_wallet);
        self.logger.debug(format!("ExpiredCoresPackage handled"));
        ()
    }
}

impl Handler<InboundServerData> for ProxyClient {
    type Result = ();

    fn handle(&mut self, msg: InboundServerData, _ctx: &mut Self::Context) -> Self::Result {
        let msg_data_len = msg.data.len();
        let msg_source = msg.source;
        let msg_sequence_number = msg.sequence_number;
        let msg_last_data = msg.last_data;
        let msg_stream_key = msg.stream_key.clone();
        let stream_context = match self.stream_contexts.get(&msg.stream_key) {
            Some(sc) => sc,
            None => {
                self.logger.error(format!(
                    "Received unsolicited {}-byte response from {}, seq {}: ignoring",
                    msg_data_len, msg_source, msg_sequence_number
                ));
                return ();
            }
        };
        if self.send_response_to_hopper(msg, &stream_context).is_err() {
            return ();
        };
        self.report_response_exit_to_accountant(&stream_context, msg_data_len);
        if msg_last_data {
            self.stream_contexts.remove(&msg_stream_key).is_some();
        }
        ()
    }
}

impl ProxyClient {
    pub fn new(cryptde: &'static dyn CryptDE, dns_servers: Vec<SocketAddr>) -> ProxyClient {
        if dns_servers.is_empty() {
            panic! ("Proxy Client requires at least one DNS server IP address after the --dns_servers parameter")
        }
        ProxyClient {
            dns_servers,
            resolver_wrapper_factory: Box::new(ResolverWrapperFactoryReal {}),
            stream_handler_pool_factory: Box::new(StreamHandlerPoolFactoryReal {}),
            cryptde,
            to_hopper: None,
            to_accountant: None,
            pool: None,
            stream_contexts: HashMap::new(),
            logger: Logger::new("Proxy Client"),
        }
    }

    pub fn make_subs_from(addr: &Addr<Syn, ProxyClient>) -> ProxyClientSubs {
        ProxyClientSubs {
            bind: addr.clone().recipient::<BindMessage>(),
            from_hopper: addr.clone().recipient::<ExpiredCoresPackage>(),
            inbound_server_data: addr.clone().recipient::<InboundServerData>(),
        }
    }

    fn send_response_to_hopper(
        &self,
        msg: InboundServerData,
        stream_context: &StreamContext,
    ) -> Result<(), ()> {
        let msg_data_len = msg.data.len() as u32;
        let msg_source = msg.source;
        let msg_sequence_number = msg.sequence_number;
        let payload = ClientResponsePayload {
            stream_key: msg.stream_key,
            sequenced_packet: SequencedPacket {
                data: msg.data,
                sequence_number: msg.sequence_number,
                last_data: msg.last_data,
            },
        };
        let icp = match IncipientCoresPackage::new(
            self.cryptde,
            stream_context.return_route.clone(),
            payload,
            &stream_context.payload_destination_key,
        ) {
            Ok(icp) => icp,
            Err(err) => {
                self.logger.error (format! ("Could not create CORES package for {}-byte response from {}, seq {}: {} - ignoring", msg_data_len, msg_source, msg_sequence_number, err));
                return Err(());
            }
        };
        self.to_hopper
            .as_ref()
            .expect("Hopper unbound")
            .try_send(icp)
            .expect("Hopper is dead");
        Ok(())
    }

    fn report_response_exit_to_accountant(
        &self,
        stream_context: &StreamContext,
        msg_data_len: usize,
    ) {
        if let Some(consuming_wallet) = stream_context.consuming_wallet.clone() {
            let exit_report = ReportExitServiceProvidedMessage {
                consuming_wallet,
                payload_size: msg_data_len,
                service_rate: TEMPORARY_PER_EXIT_RATE,
                byte_rate: TEMPORARY_PER_EXIT_BYTE_RATE,
            };
            self.to_accountant
                .as_ref()
                .expect("Accountant unbound")
                .try_send(exit_report)
                .expect("Accountant is dead");
        } else {
            self.logger.debug(format!(
                "Relayed {}-byte response without consuming wallet for free",
                msg_data_len
            ));
        }
    }
}

struct StreamContext {
    return_route: Route,
    payload_destination_key: PublicKey,
    consuming_wallet: Option<Wallet>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy_client::local_test_utils::ResolverWrapperFactoryMock;
    use crate::proxy_client::local_test_utils::ResolverWrapperMock;
    use crate::proxy_client::resolver_wrapper::ResolverWrapper;
    use crate::proxy_client::stream_handler_pool::StreamHandlerPool;
    use crate::proxy_client::stream_handler_pool::StreamHandlerPoolFactory;
    use crate::sub_lib::accountant::ReportExitServiceProvidedMessage;
    use crate::sub_lib::cryptde::encodex;
    use crate::sub_lib::cryptde::CryptData;
    use crate::sub_lib::cryptde::PublicKey;
    use crate::sub_lib::proxy_client::ClientResponsePayload;
    use crate::sub_lib::proxy_client::TEMPORARY_PER_EXIT_BYTE_RATE;
    use crate::sub_lib::proxy_client::TEMPORARY_PER_EXIT_RATE;
    use crate::sub_lib::proxy_server::ClientRequestPayload;
    use crate::sub_lib::proxy_server::ProxyProtocol;
    use crate::sub_lib::route::Route;
    use crate::sub_lib::sequence_buffer::SequencedPacket;
    use crate::sub_lib::wallet::Wallet;
    use crate::test_utils::logging::init_test_logging;
    use crate::test_utils::logging::TestLogHandler;
    use crate::test_utils::recorder::make_recorder;
    use crate::test_utils::recorder::peer_actors_builder;
    use crate::test_utils::recorder::Recorder;
    use crate::test_utils::test_utils::cryptde;
    use crate::test_utils::test_utils::make_meaningless_route;
    use crate::test_utils::test_utils::make_meaningless_stream_key;
    use crate::test_utils::test_utils::route_to_proxy_client;
    use actix::msgs;
    use actix::Arbiter;
    use actix::System;
    use std::cell::RefCell;
    use std::net::IpAddr;
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::Mutex;

    fn dnss() -> Vec<SocketAddr> {
        vec![SocketAddr::from_str("8.8.8.8:53").unwrap()]
    }

    pub struct StreamHandlerPoolMock {
        process_package_parameters: Arc<Mutex<Vec<(ClientRequestPayload, Option<Wallet>)>>>,
    }

    impl StreamHandlerPool for StreamHandlerPoolMock {
        fn process_package(&self, payload: ClientRequestPayload, consuming_wallet: Option<Wallet>) {
            self.process_package_parameters
                .lock()
                .unwrap()
                .push((payload, consuming_wallet));
        }
    }

    impl StreamHandlerPoolMock {
        pub fn new() -> StreamHandlerPoolMock {
            StreamHandlerPoolMock {
                process_package_parameters: Arc::new(Mutex::new(vec![])),
            }
        }

        pub fn process_package_parameters(
            self,
            parameters: &mut Arc<Mutex<Vec<(ClientRequestPayload, Option<Wallet>)>>>,
        ) -> StreamHandlerPoolMock {
            *parameters = self.process_package_parameters.clone();
            self
        }
    }

    pub struct StreamHandlerPoolFactoryMock {
        make_parameters: Arc<
            Mutex<
                Vec<(
                    Box<dyn ResolverWrapper>,
                    &'static dyn CryptDE,
                    Recipient<Syn, ReportExitServiceProvidedMessage>,
                    Recipient<Syn, InboundServerData>,
                )>,
            >,
        >,
        make_results: RefCell<Vec<Box<dyn StreamHandlerPool>>>,
    }

    impl StreamHandlerPoolFactory for StreamHandlerPoolFactoryMock {
        fn make(
            &self,
            resolver: Box<dyn ResolverWrapper>,
            cryptde: &'static dyn CryptDE,
            accountant_sub: Recipient<Syn, ReportExitServiceProvidedMessage>,
            proxy_client_sub: Recipient<Syn, InboundServerData>,
        ) -> Box<dyn StreamHandlerPool> {
            self.make_parameters.lock().unwrap().push((
                resolver,
                cryptde,
                accountant_sub,
                proxy_client_sub,
            ));
            self.make_results.borrow_mut().remove(0)
        }
    }

    impl StreamHandlerPoolFactoryMock {
        pub fn new() -> StreamHandlerPoolFactoryMock {
            StreamHandlerPoolFactoryMock {
                make_parameters: Arc::new(Mutex::new(vec![])),
                make_results: RefCell::new(vec![]),
            }
        }

        pub fn make_parameters(
            self,
            parameters: &mut Arc<
                Mutex<
                    Vec<(
                        Box<dyn ResolverWrapper>,
                        &'static dyn CryptDE,
                        Recipient<Syn, ReportExitServiceProvidedMessage>,
                        Recipient<Syn, InboundServerData>,
                    )>,
                >,
            >,
        ) -> StreamHandlerPoolFactoryMock {
            *parameters = self.make_parameters.clone();
            self
        }

        pub fn make_result(
            self,
            result: Box<dyn StreamHandlerPool>,
        ) -> StreamHandlerPoolFactoryMock {
            self.make_results.borrow_mut().push(result);
            self
        }
    }

    #[test]
    #[should_panic(
        expected = "Proxy Client requires at least one DNS server IP address after the --dns_servers parameter"
    )]
    fn at_least_one_dns_server_must_be_provided() {
        ProxyClient::new(cryptde(), vec![]);
    }

    #[test]
    fn bind_operates_properly() {
        let system = System::new("bind_initializes_resolver_wrapper_properly");
        let resolver_wrapper = ResolverWrapperMock::new();
        let mut resolver_wrapper_new_parameters_arc: Arc<
            Mutex<Vec<(ResolverConfig, ResolverOpts)>>,
        > = Arc::new(Mutex::new(vec![]));
        let resolver_wrapper_factory = ResolverWrapperFactoryMock::new()
            .new_parameters(&mut resolver_wrapper_new_parameters_arc)
            .new_result(Box::new(resolver_wrapper));
        let pool = StreamHandlerPoolMock::new();
        let mut pool_factory_make_parameters = Arc::new(Mutex::new(vec![]));
        let pool_factory = StreamHandlerPoolFactoryMock::new()
            .make_parameters(&mut pool_factory_make_parameters)
            .make_result(Box::new(pool));
        let peer_actors = peer_actors_builder().build();
        let mut subject = ProxyClient::new(
            cryptde(),
            vec![
                SocketAddr::from_str("4.3.2.1:4321").unwrap(),
                SocketAddr::from_str("5.4.3.2:5432").unwrap(),
            ],
        );
        subject.resolver_wrapper_factory = Box::new(resolver_wrapper_factory);
        subject.stream_handler_pool_factory = Box::new(pool_factory);
        let subject_addr: Addr<Syn, ProxyClient> = subject.start();

        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        Arbiter::system().try_send(msgs::SystemExit(0)).unwrap();
        system.run();

        let mut resolver_wrapper_new_parameters =
            resolver_wrapper_new_parameters_arc.lock().unwrap();
        let (config, opts) = resolver_wrapper_new_parameters.remove(0);
        assert_eq!(config.domain(), None);
        assert_eq!(config.search(), &[]);
        assert_eq!(
            config.name_servers(),
            &[
                NameServerConfig {
                    socket_addr: SocketAddr::from_str("4.3.2.1:4321").unwrap(),
                    protocol: Protocol::Udp,
                    tls_dns_name: None
                },
                NameServerConfig {
                    socket_addr: SocketAddr::from_str("5.4.3.2:5432").unwrap(),
                    protocol: Protocol::Udp,
                    tls_dns_name: None
                },
            ]
        );
        assert_eq!(opts, ResolverOpts::default());
        assert_eq!(resolver_wrapper_new_parameters.is_empty(), true);
    }

    #[test]
    #[should_panic(expected = "StreamHandlerPool unbound")]
    fn panics_if_unbound() {
        let request = ClientRequestPayload {
            stream_key: make_meaningless_stream_key(),
            sequenced_packet: SequencedPacket {
                data: b"HEAD http://www.nyan.cat/ HTTP/1.1\r\n\r\n".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: Some(String::from("target.hostname.com")),
            target_port: 1234,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: PublicKey::new(&b"originator_public_key"[..]),
        };
        let cryptde = cryptde();
        let package = ExpiredCoresPackage::new(
            IpAddr::from_str("1.2.3.4").unwrap(),
            Some(Wallet::new("consuming")),
            route_to_proxy_client(&cryptde.public_key(), cryptde),
            encodex(cryptde, &cryptde.public_key(), &request).unwrap(),
        );
        let system = System::new("panics_if_hopper_is_unbound");
        let subject = ProxyClient::new(cryptde, dnss());
        let subject_addr: Addr<Syn, ProxyClient> = subject.start();

        subject_addr.try_send(package).unwrap();

        Arbiter::system().try_send(msgs::SystemExit(0)).unwrap();
        system.run();
    }

    #[test]
    fn invalid_package_is_logged_and_discarded() {
        init_test_logging();
        let package = ExpiredCoresPackage::new(
            IpAddr::from_str("1.2.3.4").unwrap(),
            Some(Wallet::new("consuming")),
            make_meaningless_route(),
            CryptData::new(&b"invalid"[..]),
        );
        let system = System::new("invalid_package_is_logged_and_discarded");
        let subject = ProxyClient::new(cryptde(), dnss());
        let addr: Addr<Syn, ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder().build();
        addr.try_send(BindMessage { peer_actors }).unwrap();

        addr.try_send(package).unwrap();

        Arbiter::system().try_send(msgs::SystemExit(0)).unwrap();
        system.run();
        TestLogHandler::new()
            .exists_log_containing("ERROR: Proxy Client: Error ('Decryption error: InvalidKey");
    }

    #[test]
    fn data_from_hopper_is_relayed_to_stream_handler_pool() {
        let cryptde = cryptde();
        let request = ClientRequestPayload {
            stream_key: make_meaningless_stream_key(),
            sequenced_packet: SequencedPacket {
                data: b"inbound data".to_vec(),
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: None,
            target_port: 0,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: PublicKey::new(&b"originator"[..]),
        };
        let package = ExpiredCoresPackage::new(
            IpAddr::from_str("1.2.3.4").unwrap(),
            Some(Wallet::new("consuming")),
            make_meaningless_route(),
            encodex(cryptde, &cryptde.public_key(), &request).unwrap(),
        );
        let hopper = Recorder::new();

        let system = System::new("unparseable_request_results_in_log_and_no_response");
        let peer_actors = peer_actors_builder().hopper(hopper).build();
        let mut process_package_parameters = Arc::new(Mutex::new(vec![]));
        let pool = Box::new(
            StreamHandlerPoolMock::new()
                .process_package_parameters(&mut process_package_parameters),
        );
        let pool_factory = StreamHandlerPoolFactoryMock::new().make_result(pool);
        let resolver = ResolverWrapperMock::new()
            .lookup_ip_success(vec![IpAddr::from_str("4.3.2.1").unwrap()]);
        let resolver_factory = ResolverWrapperFactoryMock::new().new_result(Box::new(resolver));
        let mut subject = ProxyClient::new(cryptde, dnss());
        subject.resolver_wrapper_factory = Box::new(resolver_factory);
        subject.stream_handler_pool_factory = Box::new(pool_factory);
        let subject_addr: Addr<Syn, ProxyClient> = subject.start();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr.try_send(package).unwrap();

        Arbiter::system().try_send(msgs::SystemExit(0)).unwrap();
        system.run();
        let parameter = process_package_parameters.lock().unwrap().remove(0);
        assert_eq!(parameter, (request, Some(Wallet::new("consuming")),));
    }

    #[test]
    fn inbound_server_data_is_translated_to_cores_packages() {
        init_test_logging();
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = make_meaningless_stream_key();
        let data: &[u8] = b"An honest politician is one who, when he is bought, will stay bought.";
        let system = System::new("inbound_server_data_is_translated_to_cores_packages");
        let mut subject = ProxyClient::new(
            cryptde(),
            vec![SocketAddr::from_str("8.7.6.5:4321").unwrap()],
        );
        subject.stream_contexts.insert(
            stream_key.clone(),
            StreamContext {
                return_route: make_meaningless_route(),
                payload_destination_key: PublicKey::new(&b"abcd"[..]),
                consuming_wallet: Some(Wallet::new("consuming")),
            },
        );
        let subject_addr: Addr<Syn, ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .build();

        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1234,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();
        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: true,
                sequence_number: 1235,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();
        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1236,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();

        Arbiter::system().try_send(msgs::SystemExit(0)).unwrap();
        system.run();
        let hopper_recording = hopper_recording_arc.lock().unwrap();
        assert_eq!(
            hopper_recording.get_record::<IncipientCoresPackage>(0),
            &IncipientCoresPackage::new(
                cryptde(),
                make_meaningless_route(),
                ClientResponsePayload {
                    stream_key: stream_key.clone(),
                    sequenced_packet: SequencedPacket {
                        data: Vec::from(data),
                        sequence_number: 1234,
                        last_data: false
                    },
                },
                &PublicKey::new(&b"abcd"[..]),
            )
            .unwrap()
        );
        assert_eq!(
            hopper_recording.get_record::<IncipientCoresPackage>(1),
            &IncipientCoresPackage::new(
                cryptde(),
                make_meaningless_route(),
                ClientResponsePayload {
                    stream_key: stream_key.clone(),
                    sequenced_packet: SequencedPacket {
                        data: Vec::from(data),
                        sequence_number: 1235,
                        last_data: true
                    },
                },
                &PublicKey::new(&b"abcd"[..]),
            )
            .unwrap()
        );
        assert_eq!(hopper_recording.len(), 2);

        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(
            accountant_recording.get_record::<ReportExitServiceProvidedMessage>(0),
            &ReportExitServiceProvidedMessage {
                consuming_wallet: Wallet::new("consuming"),
                payload_size: data.len(),
                service_rate: TEMPORARY_PER_EXIT_RATE,
                byte_rate: TEMPORARY_PER_EXIT_BYTE_RATE
            }
        );
        assert_eq!(
            accountant_recording.get_record::<ReportExitServiceProvidedMessage>(1),
            &ReportExitServiceProvidedMessage {
                consuming_wallet: Wallet::new("consuming"),
                payload_size: data.len(),
                service_rate: TEMPORARY_PER_EXIT_RATE,
                byte_rate: TEMPORARY_PER_EXIT_BYTE_RATE
            }
        );
        assert_eq!(accountant_recording.len(), 2);
        TestLogHandler::new ().exists_log_containing(format!("ERROR: Proxy Client: Received unsolicited {}-byte response from 1.2.3.4:5678, seq 1236: ignoring", data.len ()).as_str ());
    }

    #[test]
    fn inbound_server_data_without_consuming_wallet_does_not_report_exit_service() {
        init_test_logging();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = make_meaningless_stream_key();
        let data: &[u8] = b"An honest politician is one who, when he is bought, will stay bought.";
        let system = System::new("inbound_server_data_is_translated_to_cores_packages");
        let mut subject = ProxyClient::new(
            cryptde(),
            vec![SocketAddr::from_str("8.7.6.5:4321").unwrap()],
        );
        subject.stream_contexts.insert(
            stream_key.clone(),
            StreamContext {
                return_route: make_meaningless_route(),
                payload_destination_key: PublicKey::new(&b"abcd"[..]),
                consuming_wallet: None,
            },
        );
        let subject_addr: Addr<Syn, ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder().accountant(accountant).build();

        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1234,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();

        Arbiter::system().try_send(msgs::SystemExit(0)).unwrap();
        system.run();
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(accountant_recording.len(), 0);
        TestLogHandler::new().exists_log_containing(
            format!(
                "DEBUG: Proxy Client: Relayed {}-byte response without consuming wallet for free",
                data.len()
            )
            .as_str(),
        );
    }

    #[test]
    fn error_creating_incipient_cores_package_is_logged_and_dropped() {
        init_test_logging();
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = make_meaningless_stream_key();
        let data: &[u8] = b"An honest politician is one who, when he is bought, will stay bought.";
        let system = System::new("inbound_server_data_is_translated_to_cores_packages");
        let mut subject = ProxyClient::new(
            cryptde(),
            vec![SocketAddr::from_str("8.7.6.5:4321").unwrap()],
        );
        subject.stream_contexts.insert(
            stream_key.clone(),
            StreamContext {
                return_route: make_meaningless_route(),
                payload_destination_key: PublicKey::new(&[]),
                consuming_wallet: Some(Wallet::new("consuming")),
            },
        );
        let subject_addr: Addr<Syn, ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();

        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1234,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data),
            })
            .unwrap();

        Arbiter::system().try_send(msgs::SystemExit(0)).unwrap();
        system.run();
        let hopper_recording = hopper_recording_arc.lock().unwrap();
        assert_eq!(hopper_recording.len(), 0);
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(accountant_recording.len(), 0);
        TestLogHandler::new ().exists_log_containing(format!("ERROR: Proxy Client: Could not create CORES package for {}-byte response from 1.2.3.4:5678, seq 1234: Could not encrypt payload: EmptyKey - ignoring", data.len ()).as_str ());
    }

    #[test]
    fn new_return_route_overwrites_existing_return_route() {
        let cryptde = cryptde();
        let (hopper, _, hopper_recording_arc) = make_recorder();
        let (accountant, _, accountant_recording_arc) = make_recorder();
        let stream_key = make_meaningless_stream_key();
        let data: &[u8] = b"An honest politician is one who, when he is bought, will stay bought.";
        let system = System::new("new_return_route_overwrites_existing_return_route");
        let mut subject =
            ProxyClient::new(cryptde, vec![SocketAddr::from_str("8.7.6.5:4321").unwrap()]);
        let mut process_package_params_arc = Arc::new(Mutex::new(vec![]));
        let pool = StreamHandlerPoolMock::new()
            .process_package_parameters(&mut process_package_params_arc);
        let pool_factory = StreamHandlerPoolFactoryMock::new().make_result(Box::new(pool));
        let old_return_route = Route {
            hops: vec![CryptData::new(&[1, 2, 3, 4])],
        };
        let new_return_route = make_meaningless_route();
        let originator_public_key = PublicKey::new(&[4, 3, 2, 1]);
        subject.stream_contexts.insert(
            stream_key.clone(),
            StreamContext {
                return_route: old_return_route,
                payload_destination_key: originator_public_key.clone(),
                consuming_wallet: Some(Wallet::new("consuming")),
            },
        );
        subject.stream_handler_pool_factory = Box::new(pool_factory);
        let subject_addr: Addr<Syn, ProxyClient> = subject.start();
        let peer_actors = peer_actors_builder()
            .hopper(hopper)
            .accountant(accountant)
            .build();
        subject_addr.try_send(BindMessage { peer_actors }).unwrap();
        let payload = ClientRequestPayload {
            stream_key: stream_key.clone(),
            sequenced_packet: SequencedPacket {
                data: vec![],
                sequence_number: 0,
                last_data: false,
            },
            target_hostname: None,
            target_port: 0,
            protocol: ProxyProtocol::HTTP,
            originator_public_key: originator_public_key.clone(),
        };

        subject_addr
            .try_send(ExpiredCoresPackage::new(
                IpAddr::from_str("2.3.4.5").unwrap(),
                Some(Wallet::new("gnimusnoc")),
                new_return_route.clone(),
                encodex(cryptde, &cryptde.public_key(), &payload).unwrap(),
            ))
            .unwrap();

        subject_addr
            .try_send(InboundServerData {
                stream_key: stream_key.clone(),
                last_data: false,
                sequence_number: 1234,
                source: SocketAddr::from_str("1.2.3.4:5678").unwrap(),
                data: Vec::from(data.clone()),
            })
            .unwrap();
        Arbiter::system().try_send(msgs::SystemExit(0)).unwrap();
        system.run();
        let mut process_package_params = process_package_params_arc.lock().unwrap();
        let (actual_payload, consuming_wallet_opt) = process_package_params.remove(0);
        assert_eq!(actual_payload, payload);
        assert_eq!(consuming_wallet_opt, Some(Wallet::new("gnimusnoc")));
        let hopper_recording = hopper_recording_arc.lock().unwrap();
        let expected_icp = IncipientCoresPackage::new(
            cryptde,
            new_return_route,
            ClientResponsePayload {
                stream_key,
                sequenced_packet: SequencedPacket {
                    data: Vec::from(data.clone()),
                    sequence_number: 1234,
                    last_data: false,
                },
            },
            &originator_public_key,
        )
        .unwrap();

        assert_eq!(
            hopper_recording.get_record::<IncipientCoresPackage>(0),
            &expected_icp.clone()
        );
        let accountant_recording = accountant_recording_arc.lock().unwrap();
        assert_eq!(
            accountant_recording.get_record::<ReportExitServiceProvidedMessage>(0),
            &ReportExitServiceProvidedMessage {
                consuming_wallet: Wallet::new("gnimusnoc"),
                payload_size: data.len(),
                service_rate: TEMPORARY_PER_EXIT_RATE,
                byte_rate: TEMPORARY_PER_EXIT_BYTE_RATE,
            }
        )
    }
}
