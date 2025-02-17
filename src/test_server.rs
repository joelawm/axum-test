use ::anyhow::Context;
use ::anyhow::Result;
use ::cookie::Cookie;
use ::cookie::CookieJar;
use ::http::HeaderName;
use ::http::HeaderValue;
use ::http::Method;
use ::serde::Serialize;
use ::std::sync::Arc;
use ::std::sync::Mutex;
use ::url::Url;

use crate::internals::ExpectedState;
use crate::transport_layer::IntoTransportLayer;
use crate::transport_layer::TransportLayer;
use crate::transport_layer::TransportLayerBuilder;
use crate::TestRequest;
use crate::TestRequestConfig;
use crate::TestServerConfig;
use crate::Transport;

mod server_shared_state;
pub(crate) use self::server_shared_state::*;
use crate::internals::RequestPathFormatter;

const DEFAULT_URL_ADDRESS: &'static str = "http://localhost";

///
/// The `TestServer` runs your Axum application,
/// allowing you to make HTTP requests against it.
///
/// You can make a request by calling [`TestServer::get()`](crate::TestServer::get()),
/// [`TestServer::post()`](crate::TestServer::post()), [`TestServer::put()`](crate::TestServer::put()),
/// [`TestServer::delete()`](crate::TestServer::delete()), and [`TestServer::patch()`](crate::TestServer::patch()) methods.
/// They will return a [`TestRequest`](crate::TestRequest) for building the request.
///
/// ```rust
/// # async fn test() -> Result<(), Box<dyn ::std::error::Error>> {
/// #
/// use ::axum::Json;
/// use ::axum::routing::Router;
/// use ::axum::routing::get;
/// use ::serde::Deserialize;
/// use ::serde::Serialize;
///
/// use ::axum_test::TestServer;
///
/// let app = Router::new()
///     .route(&"/todo", get(|| async { "hello!" }));
///
/// let server = TestServer::new(app)?;
///
/// // The different responses one can make:
/// let get_response = server.get(&"/todo").await;
/// let post_response = server.post(&"/todo").await;
/// let put_response = server.put(&"/todo").await;
/// let delete_response = server.delete(&"/todo").await;
/// let patch_response = server.patch(&"/todo").await;
/// #
/// # Ok(())
/// # }
/// ```
///
#[derive(Debug)]
pub struct TestServer {
    state: Arc<Mutex<ServerSharedState>>,
    transport: Arc<Mutex<Box<dyn TransportLayer>>>,
    save_cookies: bool,
    expected_state: ExpectedState,
    default_content_type: Option<String>,
    is_http_path_restricted: bool,
}

impl TestServer {
    /// This will run the given Axum app,
    /// allowing you to make requests against it.
    ///
    /// This is the same as creating a new `TestServer` with a configuration,
    /// and passing `TestServerConfig::default()`.
    ///
    pub fn new<A>(app: A) -> Result<Self>
    where
        A: IntoTransportLayer,
    {
        Self::new_with_config(app, TestServerConfig::default())
    }

    /// This very similar to [`TestServer::new()`],
    /// however you can customise some of the configuration.
    /// This includes which port to run on, or default settings.
    ///
    /// See the [`TestServerConfig`] for more information on each configuration setting.
    pub fn new_with_config<A>(app: A, config: TestServerConfig) -> Result<Self>
    where
        A: IntoTransportLayer,
    {
        let shared_state = ServerSharedState::new();
        let shared_state_mutex = Mutex::new(shared_state);
        let state = Arc::new(shared_state_mutex);

        let transport = match config.transport {
            None => {
                let builder = TransportLayerBuilder::new(None, None);
                let transport = app.into_default_transport(builder)?;
                Arc::new(Mutex::new(transport))
            }
            Some(Transport::HttpRandomPort) => {
                let builder = TransportLayerBuilder::new(None, None);
                let transport = app.into_http_transport_layer(builder)?;
                Arc::new(Mutex::new(transport))
            }
            Some(Transport::HttpIpPort { ip, port }) => {
                let builder = TransportLayerBuilder::new(ip, port);
                let transport = app.into_http_transport_layer(builder)?;
                Arc::new(Mutex::new(transport))
            }
            Some(Transport::MockHttp) => {
                let transport = app.into_mock_transport_layer()?;
                Arc::new(Mutex::new(transport))
            }
        };

        let expected_state = match config.expect_success_by_default {
            true => ExpectedState::Success,
            false => ExpectedState::None,
        };

        let this = Self {
            state,
            transport,
            save_cookies: config.save_cookies,
            expected_state,
            default_content_type: config.default_content_type,
            is_http_path_restricted: config.restrict_requests_with_http_schema,
        };

        Ok(this)
    }

    /// Creates a HTTP GET request to the path.
    pub fn get(&self, path: &str) -> TestRequest {
        self.method(Method::GET, path)
    }

    /// Creates a HTTP POST request to the given path.
    pub fn post(&self, path: &str) -> TestRequest {
        self.method(Method::POST, path)
    }

    /// Creates a HTTP PATCH request to the path.
    pub fn patch(&self, path: &str) -> TestRequest {
        self.method(Method::PATCH, path)
    }

    /// Creates a HTTP PUT request to the path.
    pub fn put(&self, path: &str) -> TestRequest {
        self.method(Method::PUT, path)
    }

    /// Creates a HTTP DELETE request to the path.
    pub fn delete(&self, path: &str) -> TestRequest {
        self.method(Method::DELETE, path)
    }

    /// Creates a HTTP request, to the method and path provided.
    pub fn method(&self, method: Method, path: &str) -> TestRequest {
        let config = self.test_request_config(method.clone(), path);
        let maybe_request = TestRequest::new(self.state.clone(), self.transport.clone(), config);

        maybe_request
            .with_context(|| {
                format!("Trying to create internal request, for request {method} {path}")
            })
            .unwrap()
    }

    /// Returns the local web address for the test server,
    /// if an address is available.
    ///
    /// The address is available when running as a real web server,
    /// by setting the [`TestServerConfig`](crate::TestServerConfig) `transport` field to `Transport::HttpRandomPort` or `Transport::HttpRandomPort`.
    ///
    /// This will return `None` when there is mock HTTP transport (the default).
    pub fn server_address(&self) -> Option<Url> {
        self.url()
    }

    /// Adds a single cookie to be included on *all* future requests.
    ///
    /// If a cookie with the same name already exists,
    /// then it will be replaced.
    pub fn add_cookie(&mut self, cookie: Cookie) {
        ServerSharedState::add_cookie(&mut self.state, cookie)
            .with_context(|| format!("Trying to call add_cookie"))
            .unwrap()
    }

    /// Adds extra cookies to be used on *all* future requests.
    ///
    /// Any cookies which have the same name as the new cookies,
    /// will get replaced.
    pub fn add_cookies(&mut self, cookies: CookieJar) {
        ServerSharedState::add_cookies(&mut self.state, cookies)
            .with_context(|| format!("Trying to call add_cookies"))
            .unwrap()
    }

    /// Clears all of the cookies stored internally.
    pub fn clear_cookies(&mut self) {
        ServerSharedState::clear_cookies(&mut self.state)
            .with_context(|| format!("Trying to call clear_cookies"))
            .unwrap()
    }

    /// Requests made using this `TestServer` will save their cookies for future requests to send.
    ///
    /// This behaviour is off by default.
    pub fn do_save_cookies(&mut self) {
        self.save_cookies = true;
    }

    /// Requests made using this `TestServer` will _not_ save their cookies for future requests to send up.
    ///
    /// This is the default behaviour.
    pub fn do_not_save_cookies(&mut self) {
        self.save_cookies = false;
    }

    /// Requests made using this `TestServer` will assert a HTTP status in the 2xx range will be returned, unless marked otherwise.
    ///
    /// By default this behaviour is off.
    pub fn expect_success(&mut self) {
        self.expected_state = ExpectedState::Success;
    }

    /// Requests made using this `TestServer` will assert a HTTP status is outside the 2xx range will be returned, unless marked otherwise.
    ///
    /// By default this behaviour is off.
    pub fn expect_failure(&mut self) {
        self.expected_state = ExpectedState::Failure;
    }

    /// Adds query parameters to be sent on *all* future requests.
    pub fn add_query_param<V>(&mut self, key: &str, value: V)
    where
        V: Serialize,
    {
        ServerSharedState::add_query_param(&mut self.state, key, value)
            .with_context(|| format!("Trying to call add_query_param"))
            .unwrap()
    }

    /// Adds query parameters to be sent with this request.
    pub fn add_query_params<V>(&mut self, query_params: V)
    where
        V: Serialize,
    {
        ServerSharedState::add_query_params(&mut self.state, query_params)
            .with_context(|| format!("Trying to call add_query_params"))
            .unwrap()
    }

    /// Clears all query params set.
    pub fn clear_query_params(&mut self) {
        ServerSharedState::clear_query_params(&mut self.state)
            .with_context(|| format!("Trying to call clear_query_params"))
            .unwrap()
    }

    /// Adds a header to be sent with all future requests built from this `TestServer`.
    pub fn add_header<'c>(&mut self, name: HeaderName, value: HeaderValue) {
        ServerSharedState::add_header(&mut self.state, name, value)
            .with_context(|| format!("Trying to call add_header"))
            .unwrap()
    }

    /// Clears all headers set so far.
    pub fn clear_headers(&mut self) {
        ServerSharedState::clear_headers(&mut self.state)
            .with_context(|| format!("Trying to call clear_headers"))
            .unwrap()
    }

    pub(crate) fn url(&self) -> Option<Url> {
        let locked = self
            .transport
            .lock()
            .expect("Failed to lock TransportLayer");

        locked.url().map(|url| url.clone())
    }

    pub(crate) fn test_request_config(&self, method: Method, path: &str) -> TestRequestConfig {
        let url = self
            .url()
            .unwrap_or_else(|| DEFAULT_URL_ADDRESS.parse().unwrap());

        TestRequestConfig {
            is_saving_cookies: self.save_cookies,
            expected_state: self.expected_state,
            content_type: self.default_content_type.clone(),
            full_request_url: build_url(url, path, self.is_http_path_restricted),
            request_format: RequestPathFormatter::new(method, path.to_string()),
        }
    }
}

fn build_url(mut url: Url, path: &str, is_http_restricted: bool) -> Url {
    if is_http_restricted {
        url.set_path(path);
        return url;
    }

    path.parse().unwrap_or_else(|_| {
        url.set_path(path);
        url
    })
}

#[cfg(test)]
mod test_new {
    use ::axum::routing::get;
    use ::axum::Router;
    use ::std::net::SocketAddr;

    use crate::TestServer;

    async fn get_ping() -> &'static str {
        "pong!"
    }

    #[tokio::test]
    async fn it_should_run_into_make_into_service_with_connect_info_by_default() {
        // Build an application with a route.
        let app = Router::new()
            .route("/ping", get(get_ping))
            .into_make_service_with_connect_info::<SocketAddr>();

        // Run the server.
        let server = TestServer::new(app).expect("Should create test server");

        // Get the request.
        server.get(&"/ping").await.assert_text(&"pong!");
    }
}

#[cfg(test)]
mod test_get {
    use super::*;

    use ::axum::routing::get;
    use ::axum::Router;
    use ::reserve_port::ReservedSocketAddr;

    async fn get_ping() -> &'static str {
        "pong!"
    }

    #[tokio::test]
    async fn it_should_get_using_relative_path_with_slash() {
        let app = Router::new().route("/ping", get(get_ping));
        let server = TestServer::new(app).expect("Should create test server");

        // Get the request _with_ slash
        server.get(&"/ping").await.assert_text(&"pong!");
    }

    #[tokio::test]
    async fn it_should_get_using_relative_path_without_slash() {
        let app = Router::new().route("/ping", get(get_ping));
        let server = TestServer::new(app).expect("Should create test server");

        // Get the request _without_ slash
        server.get(&"ping").await.assert_text(&"pong!");
    }

    #[tokio::test]
    async fn it_should_get_using_absolute_path() {
        // Build an application with a route.
        let app = Router::new().route("/ping", get(get_ping));

        // Reserve an address
        let reserved_address = ReservedSocketAddr::reserve_random_socket_addr().unwrap();
        let ip = reserved_address.ip();
        let port = reserved_address.port();

        // Run the server.
        let test_config = TestServerConfig {
            transport: Some(Transport::HttpIpPort {
                ip: Some(ip),
                port: Some(port),
            }),
            ..TestServerConfig::default()
        };
        let server = TestServer::new_with_config(app, test_config)
            .with_context(|| format!("Should create test server with address {}:{}", ip, port))
            .unwrap();

        // Get the request.
        let absolute_url = format!("http://{ip}:{port}/ping");
        let response = server.get(&absolute_url).await;

        response.assert_text(&"pong!");
        let request_path = response.request_url();
        assert_eq!(request_path.to_string(), format!("http://{ip}:{port}/ping"));
    }

    #[tokio::test]
    async fn it_should_not_get_using_absolute_path_if_restricted() {
        // Build an application with a route.
        let app = Router::new().route("/ping", get(get_ping));

        // Reserve an IP / Port
        let reserved_address = ReservedSocketAddr::reserve_random_socket_addr().unwrap();
        let ip = reserved_address.ip();
        let port = reserved_address.port();

        // Run the server.
        let test_config = TestServerConfig {
            transport: Some(Transport::HttpIpPort {
                ip: Some(ip),
                port: Some(port),
            }),
            restrict_requests_with_http_schema: true, // Key part of the test!
            ..TestServerConfig::default()
        };
        let server = TestServer::new_with_config(app, test_config)
            .with_context(|| format!("Should create test server with address {}:{}", ip, port))
            .unwrap();

        // Get the request.
        let absolute_url = format!("http://{ip}:{port}/ping");
        let response = server.get(&absolute_url).await;

        response.assert_status_not_found();
        let request_path = response.request_url();
        assert_eq!(
            request_path.to_string(),
            format!("http://{ip}:{port}/http://{ip}:{port}/ping")
        );
    }
}

#[cfg(test)]
mod test_server_address {
    use super::*;

    use ::axum::Router;
    use ::local_ip_address::local_ip;
    use ::regex::Regex;
    use ::reserve_port::ReservedPort;

    #[tokio::test]
    async fn it_should_return_address_used_from_config() {
        let reserved_port = ReservedPort::random().unwrap();
        let ip = local_ip().unwrap();
        let port = reserved_port.port();

        let config = TestServerConfig {
            transport: Some(Transport::HttpIpPort {
                ip: Some(ip),
                port: Some(port),
            }),
            ..TestServerConfig::default()
        };

        // Build an application with a route.
        let app = Router::new();
        let server = TestServer::new_with_config(app, config)
            .with_context(|| format!("Should create test server with address {}:{}", ip, port))
            .unwrap();

        let expected_ip_port = format!("http://{}:{}/", ip, reserved_port.port());
        assert_eq!(
            server.server_address().unwrap().to_string(),
            expected_ip_port
        );
    }

    #[tokio::test]
    async fn it_should_return_default_address_without_ending_slash() {
        let app = Router::new();
        let config = TestServerConfig {
            transport: Some(Transport::HttpRandomPort),
            ..TestServerConfig::default()
        };
        let server = TestServer::new_with_config(app, config).expect("Should create test server");

        let address_regex = Regex::new("^http://127\\.0\\.0\\.1:[0-9]+/$").unwrap();
        let is_match = address_regex.is_match(&server.server_address().unwrap().to_string());
        assert!(is_match);
    }
}

#[cfg(test)]
mod test_add_cookie {
    use crate::TestServer;

    use ::axum::routing::get;
    use ::axum::Router;
    use ::axum_extra::extract::cookie::CookieJar;
    use ::cookie::Cookie;

    const TEST_COOKIE_NAME: &'static str = &"test-cookie";

    async fn get_cookie(cookies: CookieJar) -> (CookieJar, String) {
        let cookie = cookies.get(&TEST_COOKIE_NAME);
        let cookie_value = cookie
            .map(|c| c.value().to_string())
            .unwrap_or_else(|| "cookie-not-found".to_string());

        (cookies, cookie_value)
    }

    #[tokio::test]
    async fn it_should_send_cookies_added_to_request() {
        let app = Router::new().route("/cookie", get(get_cookie));
        let mut server = TestServer::new(app).expect("Should create test server");

        let cookie = Cookie::new(TEST_COOKIE_NAME, "my-custom-cookie");
        server.add_cookie(cookie);

        let response_text = server.get(&"/cookie").await.text();
        assert_eq!(response_text, "my-custom-cookie");
    }
}

#[cfg(test)]
mod test_add_cookies {
    use crate::TestServer;

    use ::axum::routing::get;
    use ::axum::Router;
    use ::axum_extra::extract::cookie::CookieJar as AxumCookieJar;
    use ::cookie::Cookie;
    use ::cookie::CookieJar;

    async fn route_get_cookies(cookies: AxumCookieJar) -> String {
        let mut all_cookies = cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<String>>();
        all_cookies.sort();

        all_cookies.join(&", ")
    }

    #[tokio::test]
    async fn it_should_send_all_cookies_added_by_jar() {
        let app = Router::new().route("/cookies", get(route_get_cookies));
        let mut server = TestServer::new(app).expect("Should create test server");

        // Build cookies to send up
        let cookie_1 = Cookie::new("first-cookie", "my-custom-cookie");
        let cookie_2 = Cookie::new("second-cookie", "other-cookie");
        let mut cookie_jar = CookieJar::new();
        cookie_jar.add(cookie_1);
        cookie_jar.add(cookie_2);

        server.add_cookies(cookie_jar);

        server
            .get(&"/cookies")
            .await
            .assert_text("first-cookie=my-custom-cookie, second-cookie=other-cookie");
    }
}

#[cfg(test)]
mod test_clear_cookies {
    use crate::TestServer;

    use ::axum::routing::get;
    use ::axum::Router;
    use ::axum_extra::extract::cookie::CookieJar as AxumCookieJar;
    use ::cookie::Cookie;
    use ::cookie::CookieJar;

    async fn route_get_cookies(cookies: AxumCookieJar) -> String {
        let mut all_cookies = cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
            .collect::<Vec<String>>();
        all_cookies.sort();

        all_cookies.join(&", ")
    }

    #[tokio::test]
    async fn it_should_not_send_cookies_cleared() {
        let app = Router::new().route("/cookies", get(route_get_cookies));
        let mut server = TestServer::new(app).expect("Should create test server");

        let cookie_1 = Cookie::new("first-cookie", "my-custom-cookie");
        let cookie_2 = Cookie::new("second-cookie", "other-cookie");
        let mut cookie_jar = CookieJar::new();
        cookie_jar.add(cookie_1);
        cookie_jar.add(cookie_2);

        server.add_cookies(cookie_jar);

        // The important bit of this test
        server.clear_cookies();

        server.get(&"/cookies").await.assert_text("");
    }
}

#[cfg(test)]
mod test_add_header {
    use super::*;

    use ::axum::async_trait;
    use ::axum::extract::FromRequestParts;
    use ::axum::routing::get;
    use ::axum::Router;
    use ::http::request::Parts;
    use ::http::HeaderName;
    use ::http::HeaderValue;
    use ::hyper::StatusCode;
    use ::std::marker::Sync;

    use crate::TestServer;

    const TEST_HEADER_NAME: &'static str = &"test-header";
    const TEST_HEADER_CONTENT: &'static str = &"Test header content";

    struct TestHeader(Vec<u8>);

    #[async_trait]
    impl<S: Sync> FromRequestParts<S> for TestHeader {
        type Rejection = (StatusCode, &'static str);

        async fn from_request_parts(
            parts: &mut Parts,
            _state: &S,
        ) -> Result<TestHeader, Self::Rejection> {
            parts
                .headers
                .get(HeaderName::from_static(TEST_HEADER_NAME))
                .map(|v| TestHeader(v.as_bytes().to_vec()))
                .ok_or((StatusCode::BAD_REQUEST, "Missing test header"))
        }
    }

    async fn ping_header(TestHeader(header): TestHeader) -> Vec<u8> {
        header
    }

    #[tokio::test]
    async fn it_should_send_header_added_to_server() {
        // Build an application with a route.
        let app = Router::new().route("/header", get(ping_header));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_header(
            HeaderName::from_static(TEST_HEADER_NAME),
            HeaderValue::from_static(TEST_HEADER_CONTENT),
        );

        // Send a request with the header
        let response = server.get(&"/header").await;

        // Check it sent back the right text
        response.assert_text(TEST_HEADER_CONTENT)
    }
}

#[cfg(test)]
mod test_clear_headers {
    use super::*;

    use ::axum::async_trait;
    use ::axum::extract::FromRequestParts;
    use ::axum::routing::get;
    use ::axum::Router;
    use ::http::request::Parts;
    use ::http::HeaderName;
    use ::http::HeaderValue;
    use ::hyper::StatusCode;
    use ::std::marker::Sync;

    use crate::TestServer;

    const TEST_HEADER_NAME: &'static str = &"test-header";
    const TEST_HEADER_CONTENT: &'static str = &"Test header content";

    struct TestHeader(Vec<u8>);

    #[async_trait]
    impl<S: Sync> FromRequestParts<S> for TestHeader {
        type Rejection = (StatusCode, &'static str);

        async fn from_request_parts(
            parts: &mut Parts,
            _state: &S,
        ) -> Result<TestHeader, Self::Rejection> {
            parts
                .headers
                .get(HeaderName::from_static(TEST_HEADER_NAME))
                .map(|v| TestHeader(v.as_bytes().to_vec()))
                .ok_or((StatusCode::BAD_REQUEST, "Missing test header"))
        }
    }

    async fn ping_header(TestHeader(header): TestHeader) -> Vec<u8> {
        header
    }

    #[tokio::test]
    async fn it_should_not_send_headers_cleared_by_server() {
        // Build an application with a route.
        let app = Router::new().route("/header", get(ping_header));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_header(
            HeaderName::from_static(TEST_HEADER_NAME),
            HeaderValue::from_static(TEST_HEADER_CONTENT),
        );
        server.clear_headers();

        // Send a request with the header
        let response = server.get(&"/header").await;

        // Check it sent back the right text
        response.assert_status_bad_request();
        response.assert_text("Missing test header");
    }
}

#[cfg(test)]
mod test_add_query_params {
    use ::axum::extract::Query;
    use ::axum::routing::get;
    use ::axum::Router;

    use ::serde::Deserialize;
    use ::serde::Serialize;
    use ::serde_json::json;

    use crate::TestServer;

    #[derive(Debug, Deserialize, Serialize)]
    struct QueryParam {
        message: String,
    }

    async fn get_query_param(Query(params): Query<QueryParam>) -> String {
        params.message
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct QueryParam2 {
        message: String,
        other: String,
    }

    async fn get_query_param_2(Query(params): Query<QueryParam2>) -> String {
        format!("{}-{}", params.message, params.other)
    }

    #[tokio::test]
    async fn it_should_pass_up_query_params_from_serialization() {
        // Build an application with a route.
        let app = Router::new().route("/query", get(get_query_param));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_params(QueryParam {
            message: "it works".to_string(),
        });

        // Get the request.
        server.get(&"/query").await.assert_text(&"it works");
    }

    #[tokio::test]
    async fn it_should_pass_up_query_params_from_pairs() {
        // Build an application with a route.
        let app = Router::new().route("/query", get(get_query_param));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_params(&[("message", "it works")]);

        // Get the request.
        server.get(&"/query").await.assert_text(&"it works");
    }

    #[tokio::test]
    async fn it_should_pass_up_multiple_query_params_from_multiple_params() {
        // Build an application with a route.
        let app = Router::new().route("/query-2", get(get_query_param_2));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_params(&[("message", "it works"), ("other", "yup")]);

        // Get the request.
        server.get(&"/query-2").await.assert_text(&"it works-yup");
    }

    #[tokio::test]
    async fn it_should_pass_up_multiple_query_params_from_multiple_calls() {
        // Build an application with a route.
        let app = Router::new().route("/query-2", get(get_query_param_2));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_params(&[("message", "it works")]);
        server.add_query_params(&[("other", "yup")]);

        // Get the request.
        server.get(&"/query-2").await.assert_text(&"it works-yup");
    }

    #[tokio::test]
    async fn it_should_pass_up_multiple_query_params_from_json() {
        // Build an application with a route.
        let app = Router::new().route("/query-2", get(get_query_param_2));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_params(json!({
            "message": "it works",
            "other": "yup"
        }));

        // Get the request.
        server.get(&"/query-2").await.assert_text(&"it works-yup");
    }
}

#[cfg(test)]
mod test_add_query_param {
    use ::axum::extract::Query;
    use ::axum::routing::get;
    use ::axum::Router;

    use ::serde::Deserialize;
    use ::serde::Serialize;

    use crate::TestServer;

    #[derive(Debug, Deserialize, Serialize)]
    struct QueryParam {
        message: String,
    }

    async fn get_query_param(Query(params): Query<QueryParam>) -> String {
        params.message
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct QueryParam2 {
        message: String,
        other: String,
    }

    async fn get_query_param_2(Query(params): Query<QueryParam2>) -> String {
        format!("{}-{}", params.message, params.other)
    }

    #[tokio::test]
    async fn it_should_pass_up_query_params_from_pairs() {
        // Build an application with a route.
        let app = Router::new().route("/query", get(get_query_param));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_param("message", "it works");

        // Get the request.
        server.get(&"/query").await.assert_text(&"it works");
    }

    #[tokio::test]
    async fn it_should_pass_up_multiple_query_params_from_multiple_calls() {
        // Build an application with a route.
        let app = Router::new().route("/query-2", get(get_query_param_2));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_param("message", "it works");
        server.add_query_param("other", "yup");

        // Get the request.
        server.get(&"/query-2").await.assert_text(&"it works-yup");
    }

    #[tokio::test]
    async fn it_should_pass_up_multiple_query_params_from_calls_across_server_and_request() {
        // Build an application with a route.
        let app = Router::new().route("/query-2", get(get_query_param_2));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_param("message", "it works");

        // Get the request.
        server
            .get(&"/query-2")
            .add_query_param("other", "yup")
            .await
            .assert_text(&"it works-yup");
    }
}

#[cfg(test)]
mod test_clear_query_params {
    use ::axum::extract::Query;
    use ::axum::routing::get;
    use ::axum::Router;

    use ::serde::Deserialize;
    use ::serde::Serialize;

    use crate::TestServer;

    #[derive(Debug, Deserialize, Serialize)]
    struct QueryParams {
        first: Option<String>,
        second: Option<String>,
    }

    async fn get_query_params(Query(params): Query<QueryParams>) -> String {
        format!(
            "has first? {}, has second? {}",
            params.first.is_some(),
            params.second.is_some()
        )
    }

    #[tokio::test]
    async fn it_should_clear_all_params_set() {
        // Build an application with a route.
        let app = Router::new().route("/query", get(get_query_params));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_params(QueryParams {
            first: Some("first".to_string()),
            second: Some("second".to_string()),
        });
        server.clear_query_params();

        // Get the request.
        server
            .get(&"/query")
            .await
            .assert_text(&"has first? false, has second? false");
    }

    #[tokio::test]
    async fn it_should_clear_all_params_set_and_allow_replacement() {
        // Build an application with a route.
        let app = Router::new().route("/query", get(get_query_params));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.add_query_params(QueryParams {
            first: Some("first".to_string()),
            second: Some("second".to_string()),
        });
        server.clear_query_params();
        server.add_query_params(QueryParams {
            first: Some("first".to_string()),
            second: Some("second".to_string()),
        });

        // Get the request.
        server
            .get(&"/query")
            .await
            .assert_text(&"has first? true, has second? true");
    }
}

#[cfg(test)]
mod test_expect_success_by_default {
    use super::*;

    use ::axum::routing::get;
    use ::axum::Router;

    #[tokio::test]
    async fn it_should_not_panic_by_default_if_accessing_404_route() {
        let app = Router::new();
        let server = TestServer::new(app).expect("Should create test server");

        server.get(&"/some_unknown_route").await;
    }

    #[tokio::test]
    async fn it_should_not_panic_by_default_if_accessing_200_route() {
        let app = Router::new().route("/known_route", get(|| async { "🦊🦊🦊" }));
        let server = TestServer::new(app).expect("Should create test server");

        server.get(&"/known_route").await;
    }

    #[tokio::test]
    #[should_panic]
    async fn it_should_panic_by_default_if_accessing_404_route_and_expect_success_on() {
        let app = Router::new();
        let server = TestServer::new_with_config(
            app,
            TestServerConfig {
                expect_success_by_default: true,
                ..TestServerConfig::default()
            },
        )
        .expect("Should create test server");

        server.get(&"/some_unknown_route").await;
    }

    #[tokio::test]
    async fn it_should_not_panic_by_default_if_accessing_200_route_and_expect_success_on() {
        let app = Router::new().route("/known_route", get(|| async { "🦊🦊🦊" }));
        let server = TestServer::new_with_config(
            app,
            TestServerConfig {
                expect_success_by_default: true,
                ..TestServerConfig::default()
            },
        )
        .expect("Should create test server");

        server.get(&"/known_route").await;
    }
}

#[cfg(test)]
mod test_content_type {
    use super::*;

    use ::axum::routing::get;
    use ::axum::Router;
    use ::http::header::CONTENT_TYPE;
    use ::http::HeaderMap;

    async fn get_content_type(headers: HeaderMap) -> String {
        headers
            .get(CONTENT_TYPE)
            .map(|h| h.to_str().unwrap().to_string())
            .unwrap_or_else(|| "".to_string())
    }

    #[tokio::test]
    async fn it_should_default_to_server_content_type_when_present() {
        // Build an application with a route.
        let app = Router::new().route("/content_type", get(get_content_type));

        // Run the server.
        let config = TestServerConfig {
            default_content_type: Some("text/plain".to_string()),
            ..TestServerConfig::default()
        };
        let server = TestServer::new_with_config(app, config).expect("Should create test server");

        // Get the request.
        let text = server.get(&"/content_type").await.text();

        assert_eq!(text, "text/plain");
    }
}

#[cfg(test)]
mod test_expect_success {
    use crate::TestServer;
    use ::axum::routing::get;
    use ::axum::Router;
    use ::http::StatusCode;

    #[tokio::test]
    async fn it_should_not_panic_if_success_is_returned() {
        async fn get_ping() -> &'static str {
            "pong!"
        }

        // Build an application with a route.
        let app = Router::new().route("/ping", get(get_ping));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.expect_success();

        // Get the request.
        server.get(&"/ping").await;
    }

    #[tokio::test]
    async fn it_should_not_panic_on_other_2xx_status_code() {
        async fn get_accepted() -> StatusCode {
            StatusCode::ACCEPTED
        }

        // Build an application with a route.
        let app = Router::new().route("/accepted", get(get_accepted));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.expect_success();

        // Get the request.
        server.get(&"/accepted").await;
    }

    #[tokio::test]
    #[should_panic]
    async fn it_should_panic_on_404() {
        // Build an application with a route.
        let app = Router::new();

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.expect_success();

        // Get the request.
        server.get(&"/some_unknown_route").await;
    }
}

#[cfg(test)]
mod test_expect_failure {
    use crate::TestServer;
    use ::axum::routing::get;
    use ::axum::Router;
    use ::http::StatusCode;

    #[tokio::test]
    async fn it_should_not_panic_if_expect_failure_on_404() {
        // Build an application with a route.
        let app = Router::new();

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.expect_failure();

        // Get the request.
        server.get(&"/some_unknown_route").await;
    }

    #[tokio::test]
    #[should_panic]
    async fn it_should_panic_if_success_is_returned() {
        async fn get_ping() -> &'static str {
            "pong!"
        }

        // Build an application with a route.
        let app = Router::new().route("/ping", get(get_ping));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.expect_failure();

        // Get the request.
        server.get(&"/ping").await;
    }

    #[tokio::test]
    #[should_panic]
    async fn it_should_panic_on_other_2xx_status_code() {
        async fn get_accepted() -> StatusCode {
            StatusCode::ACCEPTED
        }

        // Build an application with a route.
        let app = Router::new().route("/accepted", get(get_accepted));

        // Run the server.
        let mut server = TestServer::new(app).expect("Should create test server");
        server.expect_failure();

        // Get the request.
        server.get(&"/accepted").await;
    }
}
