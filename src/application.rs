use crate::api::metrics::metrics_scope;
use crate::api::file_storage::file_storage_scope;
use crate::api::user::user_scope;
use crate::api::workspace::{collab_scope, workspace_scope};
use crate::api::ws::ws_scope;
use crate::biz::casbin::access_control::{enable_access_control, AccessControl};
use crate::biz::casbin::RealtimeCollabAccessControlImpl;
use crate::biz::collab::access_control::{
    CollabMiddlewareAccessControl, CollabStorageAccessControlImpl,
};
use crate::biz::collab::cache::CollabCache;
use crate::biz::casbin::pg_listen::{
    spawn_listen_on_collab_member_change, spawn_listen_on_workspace_member_change,
};
use crate::biz::collab::storage::CollabStorageImpl;
use crate::biz::pg_listener::PgListeners;
use crate::biz::snapshot::SnapshotControl;
use crate::biz::user::RealtimeUserImpl;
use crate::biz::workspace::access_control::WorkspaceMiddlewareAccessControl;
use crate::config::config::{Config, DatabaseSetting, GoTrueSetting, S3Setting};
use crate::middleware::access_control_mw::MiddlewareAccessControlTransform;
use crate::middleware::metrics_mw::MetricsMiddleware;
use crate::middleware::request_id::RequestIdMiddleware;
use crate::self_signed::create_self_signed_certificate;
use crate::state::{AppMetrics, AppState, GoTrueAdmin, UserCache};
use actix::Actor;
use actix_identity::IdentityMiddleware;
use actix_session::storage::RedisSessionStore;
use actix_session::SessionMiddleware;
use actix_web::cookie::Key;
use actix_web::{dev::Server, web, web::Data, App, HttpServer};
use anyhow::{Context, Error};
use database::file::bucket_s3_impl::S3BucketStorage;
use openssl::ssl::{SslAcceptor, SslAcceptorBuilder, SslFiletype, SslMethod};
use openssl::x509::X509;
use realtime::server::{RTCommandReceiver, RTCommandSender, RealtimeServer};
use secrecy::{ExposeSecret, Secret};
use snowflake::Snowflake;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::info;

pub struct Application {
    port: u16,
    server: Server,
}

impl Application {
    pub async fn build(
        config: Config,
        state: AppState,
        rt_cmd_recv: RTCommandReceiver,
    ) -> Result<Self, anyhow::Error> {
        let address = format!("{}:{}", config.application.host, config.application.port);
        let listener = TcpListener::bind(&address)?;
        let port = listener.local_addr().unwrap().port();
        let server = run(listener, state, config, rt_cmd_recv).await?;
        Ok(Self { port, server })
    }
    pub async fn run_until_stopped(self) -> Result<(), std::io::Error> {
        self.server.await
    }
    pub fn port(&self) -> u16 {
        self.port
    }
}

pub async fn run(
    listener: TcpListener,
    state: AppState,
    config: Config,
    rt_cmd_recv: RTCommandReceiver,
) -> Result<Server, anyhow::Error> {
    let redis_store = RedisSessionStore::new(config.redis_uri.expose_secret())
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to connect to Redis at {:?}: {:?}",
                config.redis_uri,
                e
            )
        })?;
    let pair = get_certificate_and_server_key(&config);
    let key = pair
        .as_ref()
        .map(|(_, server_key)| Key::from(server_key.export_secret().as_bytes()))
        .unwrap_or_else(Key::generate);
    let storage = state.collab_access_control_storage.clone();
    let access_control = MiddlewareAccessControlTransform::new()
        .with_acs(WorkspaceMiddlewareAccessControl::new(
        state.pg_pool.clone(),
        state.workspace_access_control.clone().into(),
        ))
        .with_acs(CollabMiddlewareAccessControl::new(
        state.collab_access_control.clone().into(),
        state.collab_cache.clone(),
        ));
    
    // Initialize metrics that which are registered in the registry.
    let realtime_server = RealtimeServer::<_, Arc<RealtimeUserImpl>, _>::new(
        storage.clone(),
        RealtimeCollabAccessControlImpl::new(state.access_control.clone()),
        state.metrics.realtime_metrics.clone(),
        rt_cmd_recv,
    )
    .unwrap()
    .start();

    let mut server = HttpServer::new(move || {
        App::new()
            // Middleware is registered for each App, scope, or Resource and executed in opposite order as registration
            .wrap(MetricsMiddleware)
            .wrap(IdentityMiddleware::default())
            .wrap(
                SessionMiddleware::builder(redis_store.clone(), key.clone())
                .build(),
            )
            .wrap(access_control.clone())
            .wrap(RequestIdMiddleware)
            .app_data(web::JsonConfig::default().limit(5 * 1024 * 1024))
            .service(user_scope())
            .service(workspace_scope())
            .service(collab_scope())
            .service(ws_scope())
            .service(file_storage_scope())
            .service(metrics_scope())
            .app_data(Data::new(state.metrics.registry.clone()))
            .app_data(Data::new(state.metrics.request_metrics.clone()))
            .app_data(Data::new(state.metrics.realtime_metrics.clone()))
            .app_data(Data::new(state.metrics.access_control_metrics.clone()))
            .app_data(Data::new(realtime_server.clone()))
            .app_data(Data::new(state.clone()))
            .app_data(Data::new(storage.clone()))
    });
    server = match pair {
        None => server.listen(listener)?,
        Some((certificate, _)) => {
            server.listen_openssl(listener, make_ssl_acceptor_builder(certificate))?
        },
    };
    Ok(server.run())
}
