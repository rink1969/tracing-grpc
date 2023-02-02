use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::hi_client::HiClient;
use hello_world::hi_server::{Hi, HiServer};
use hello_world::{HelloReply, HelloRequest};
use hello_world::{HiReply, HiRequest};
use opentelemetry::sdk::propagation::TraceContextPropagator;
use opentelemetry::{global, propagation::Extractor, propagation::Injector};
use tonic::{transport::Server, Request, Response, Status};
use tracing::*;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::prelude::*;

#[allow(clippy::derive_partial_eq_without_eq)] // tonic don't derive Eq for generated types. We shouldn't manually change it.
pub mod hello_world {
    tonic::include_proto!("helloworld"); // The string specified here must match the proto package name
}

struct MetadataMap<'a>(&'a tonic::metadata::MetadataMap);

impl<'a> Extractor for MetadataMap<'a> {
    /// Get a value for a key from the MetadataMap.  If the value can't be converted to &str, returns None
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|metadata| metadata.to_str().ok())
    }

    /// Collect all the keys from the MetadataMap.
    fn keys(&self) -> Vec<&str> {
        self.0
            .keys()
            .map(|key| match key {
                tonic::metadata::KeyRef::Ascii(v) => v.as_str(),
                tonic::metadata::KeyRef::Binary(v) => v.as_str(),
            })
            .collect::<Vec<_>>()
    }
}

struct MutMetadataMap<'a>(&'a mut tonic::metadata::MetadataMap);

impl<'a> Injector for MutMetadataMap<'a> {
    /// Set a key and value in the MetadataMap.  Does nothing if the key or value are not valid inputs
    fn set(&mut self, key: &str, value: String) {
        if let Ok(key) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes()) {
            if let Ok(val) = std::str::FromStr::from_str(&value) {
                self.0.insert(key, val);
            }
        }
    }
}

#[instrument]
fn expensive_fn(to_print: String) {
    std::thread::sleep(std::time::Duration::from_millis(20));
    info!("{}", to_print);
}

#[derive(Debug, Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    #[instrument]
    async fn say_hello(
        &self,
        request: Request<HelloRequest>, // Accept request of type HelloRequest
    ) -> Result<Response<HelloReply>, Status> {
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        tracing::Span::current().set_parent(parent_cx);

        let name = request.into_inner().name;
        expensive_fn(format!("Got name: {name:?}"));

        let mut client = HiClient::connect("http://[::1]:50051").await.unwrap();
        let mut hi_request = tonic::Request::new(HiRequest {
            name: "Tonic".into(),
        });
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(
                &tracing::Span::current().context(),
                &mut MutMetadataMap(hi_request.metadata_mut()),
            )
        });
        let _ = client.say_hi(hi_request).await?;

        // Return an instance of type HelloReply
        let reply = hello_world::HelloReply {
            message: format!("Hello {name}!"), // We must use .into_inner() as the fields of gRPC requests and responses are private
        };

        Ok(Response::new(reply)) // Send back our formatted greeting
    }
}

#[derive(Debug, Default)]
pub struct MyHi {}

#[tonic::async_trait]
impl Hi for MyHi {
    #[instrument]
    async fn say_hi(
        &self,
        request: Request<HiRequest>, // Accept request of type HelloRequest
    ) -> Result<Response<HiReply>, Status> {
        let parent_cx =
            global::get_text_map_propagator(|prop| prop.extract(&MetadataMap(request.metadata())));
        tracing::Span::current().set_parent(parent_cx);

        info!("sleep 2s");
        std::thread::sleep(std::time::Duration::new(2, 0));
        info!("after sleep 2s");

        // Return an instance of type HiReply
        let reply = hello_world::HiReply {
            message: format!("Hi {}!", request.into_inner().name), // We must use .into_inner() as the fields of gRPC requests and responses are private
        };

        Ok(Response::new(reply)) // Send back our formatted greeting
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    global::set_text_map_propagator(TraceContextPropagator::new());
    let tracer = opentelemetry_jaeger::new_agent_pipeline()
        .with_service_name("grpc-server")
        .install_batch(opentelemetry::runtime::Tokio)?;

    // Log all events to a rolling log file.
    let logfile = tracing_appender::rolling::hourly("./logs", "myapp-logs");
    // Log `INFO` and above to stdout.
    let stdout = std::io::stdout.with_max_level(tracing::Level::INFO);

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("INFO"))
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(tracing_subscriber::fmt::layer().with_writer(stdout.and(logfile)))
        .try_init()?;

    let addr = "[::1]:50051".parse()?;
    let greeter = MyGreeter::default();
    let hi = MyHi::default();

    Server::builder()
        .add_service(GreeterServer::new(greeter))
        .add_service(HiServer::new(hi))
        .serve(addr)
        .await?;

    opentelemetry::global::shutdown_tracer_provider();

    Ok(())
}
