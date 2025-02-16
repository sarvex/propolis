// Required for USDT
#![cfg_attr(usdt_need_asm, feature(asm))]
#![cfg_attr(all(target_os = "macos", usdt_need_asm_sym), feature(asm_sym))]
#![cfg_attr(feature = "mock-only", allow(unused))]

use anyhow::{anyhow, Context};
use clap::Parser;
use dropshot::{
    ConfigDropshot, ConfigLogging, ConfigLoggingLevel, HttpServerStarter,
};
use futures::join;
use propolis::usdt::register_probes;
use slog::info;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "mock-only")]
use propolis_server::mock_server as server;

cfg_if::cfg_if! {
    if #[cfg(not(feature = "mock-only"))] {
        use propolis_server::server::{self, MetricsEndpointConfig};
        use propolis_server::vnc::setup_vnc;
    }
}

use propolis_server::config;

#[derive(Debug, Parser)]
#[clap(about, version)]
/// An HTTP server providing access to Propolis
enum Args {
    /// Generates the OpenAPI specification.
    OpenApi,
    /// Runs the Propolis server.
    Run {
        #[clap(action)]
        cfg: PathBuf,

        #[clap(name = "PROPOLIS_IP:PORT", action)]
        propolis_addr: SocketAddr,

        /// IP:Port for the Oximeter register address, which is Nexus.
        #[clap(long, action)]
        metric_addr: Option<SocketAddr>,

        #[clap(
            name = "VNC_IP:PORT",
            default_value_t = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 5900),
            action
        )]
        vnc_addr: SocketAddr,
    },
}

pub fn run_openapi() -> Result<(), String> {
    server::api()
        .openapi("Oxide Propolis Server API", "0.0.1")
        .description(
            "API for interacting with the Propolis hypervisor frontend.",
        )
        .contact_url("https://oxide.computer")
        .contact_email("api@oxide.computer")
        .write(&mut std::io::stdout())
        .map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Ensure proper setup of USDT probes
    register_probes().unwrap();

    // Command line arguments.
    let args = Args::parse();

    match args {
        Args::OpenApi => run_openapi()
            .map_err(|e| anyhow!("Cannot generate OpenAPI spec: {}", e)),
        Args::Run { cfg, propolis_addr, metric_addr, vnc_addr } => {
            let config = config::parse(&cfg)?;

            // Dropshot configuration.
            let config_dropshot = ConfigDropshot {
                bind_address: propolis_addr,
                request_body_max_bytes: 1024 * 1024, // 1M for ISO bytes
                ..Default::default()
            };
            let config_logging = ConfigLogging::StderrTerminal {
                level: ConfigLoggingLevel::Info,
            };
            let log = config_logging.to_logger("propolis-server").map_err(
                |error| anyhow!("failed to create logger: {}", error),
            )?;

            let context;
            cfg_if::cfg_if! {
                if #[cfg(not(feature = "mock-only"))] {
                    // Check that devices conform to expected API version
                    propolis::api_version::check().context("API version checks")?;

                    let vnc_server = setup_vnc(&log, vnc_addr);
                    let vnc_server_hdl = vnc_server.clone();
                    let use_reservoir = config::reservoir_decide(&log);

                    let metric_config = metric_addr.map(|addr| {
                        let imc = MetricsEndpointConfig::new(propolis_addr, addr);
                        info!(log, "Metrics server will use {:?}", imc);
                        imc
                    });
                    context = server::DropshotEndpointContext::new(
                        config,
                        vnc_server,
                        use_reservoir,
                        log.new(slog::o!()),
                        metric_config,
                    );
                } else {
                    context = server::Context::new(config, log.new(slog::o!()));
                }
            }

            info!(log, "Starting server...");

            let server = HttpServerStarter::new(
                &config_dropshot,
                server::api(),
                Arc::new(context),
                &log,
            )
            .map_err(|error| anyhow!("Failed to start server: {}", error))?
            .start();

            let server_res;
            cfg_if::cfg_if! {
                if #[cfg(not(feature = "mock-only"))] {
                    server_res = join!(server, vnc_server_hdl.start()).0;
                } else {
                    server_res = server.await;
                }
            };
            server_res
                .map_err(|e| anyhow!("Server exited with an error: {}", e))
        }
    }
}
