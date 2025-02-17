use ::anyhow::Context;
use ::anyhow::Result;
use ::axum::routing::IntoMakeService;
use ::axum::Router;
use ::axum::Server as AxumServer;
use ::tokio::spawn;
use ::url::Url;

use super::IntoTransportLayer;
use crate::internals::HttpTransportLayer;
use crate::internals::MockTransportLayer;
use crate::transport_layer::TransportLayer;
use crate::transport_layer::TransportLayerBuilder;

impl IntoTransportLayer for IntoMakeService<Router> {
    fn into_http_transport_layer(
        self,
        builder: TransportLayerBuilder,
    ) -> Result<Box<dyn TransportLayer>> {
        let (socket_addr, tcp_listener, maybe_reserved_port) =
            builder.tcp_listener_with_reserved_port()?;

        let maybe_local_address = tcp_listener.local_addr().ok();
        let server_builder = AxumServer::from_tcp(tcp_listener)
            .with_context(|| format!("Failed to create ::axum::Server for TestServer, with address '{maybe_local_address:?}'"))?;

        let server = server_builder.serve(self);
        let server_handle = spawn(async move {
            server.await.expect("Expect server to start serving");
        });

        let server_address = format!("http://{socket_addr}");
        let server_url: Url = server_address.parse()?;

        Ok(Box::new(HttpTransportLayer::new(
            server_handle,
            maybe_reserved_port,
            server_url,
        )))
    }

    fn into_mock_transport_layer(self) -> Result<Box<dyn TransportLayer>> {
        let transport_layer = MockTransportLayer::new(self);
        Ok(Box::new(transport_layer))
    }
}

#[cfg(test)]
mod test_into_http_transport_layer_for_into_make_service {
    use ::axum::extract::State;
    use ::axum::routing::get;
    use ::axum::routing::IntoMakeService;
    use ::axum::Router;

    use crate::TestServer;
    use crate::TestServerConfig;
    use crate::Transport;

    async fn get_ping() -> &'static str {
        "pong!"
    }

    async fn get_state(State(count): State<u32>) -> String {
        format!("count is {}", count)
    }

    #[tokio::test]
    async fn it_should_create_and_test_with_make_into_service() {
        // Build an application with a route.
        let app: IntoMakeService<Router> = Router::new()
            .route("/ping", get(get_ping))
            .into_make_service();

        // Run the server.
        let config = TestServerConfig {
            transport: Some(Transport::HttpRandomPort),
            ..TestServerConfig::default()
        };
        let server = TestServer::new_with_config(app, config).expect("Should create test server");

        // Get the request.
        server.get(&"/ping").await.assert_text(&"pong!");
    }

    #[tokio::test]
    async fn it_should_create_and_test_with_make_into_service_with_state() {
        // Build an application with a route.
        let app: IntoMakeService<Router> = Router::new()
            .route("/count", get(get_state))
            .with_state(123)
            .into_make_service();

        // Run the server.
        let config = TestServerConfig {
            transport: Some(Transport::HttpRandomPort),
            ..TestServerConfig::default()
        };
        let server = TestServer::new_with_config(app, config).expect("Should create test server");

        // Get the request.
        server.get(&"/count").await.assert_text(&"count is 123");
    }
}

#[cfg(test)]
mod test_into_mock_transport_layer_for_into_make_service {
    use ::axum::extract::State;
    use ::axum::routing::get;
    use ::axum::routing::IntoMakeService;
    use ::axum::Router;

    use crate::TestServer;
    use crate::TestServerConfig;
    use crate::Transport;

    async fn get_ping() -> &'static str {
        "pong!"
    }

    async fn get_state(State(count): State<u32>) -> String {
        format!("count is {}", count)
    }

    #[tokio::test]
    async fn it_should_create_and_test_with_make_into_service() {
        // Build an application with a route.
        let app: IntoMakeService<Router> = Router::new()
            .route("/ping", get(get_ping))
            .into_make_service();

        // Run the server.
        let config = TestServerConfig {
            transport: Some(Transport::MockHttp),
            ..TestServerConfig::default()
        };
        let server = TestServer::new_with_config(app, config).expect("Should create test server");

        // Get the request.
        server.get(&"/ping").await.assert_text(&"pong!");
    }

    #[tokio::test]
    async fn it_should_create_and_test_with_make_into_service_with_state() {
        // Build an application with a route.
        let app: IntoMakeService<Router> = Router::new()
            .route("/count", get(get_state))
            .with_state(123)
            .into_make_service();

        // Run the server.
        let config = TestServerConfig {
            transport: Some(Transport::MockHttp),
            ..TestServerConfig::default()
        };
        let server = TestServer::new_with_config(app, config).expect("Should create test server");

        // Get the request.
        server.get(&"/count").await.assert_text(&"count is 123");
    }
}
