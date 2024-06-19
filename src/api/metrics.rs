use actix_web::web;
use actix_web::HttpResponse;
use actix_web::Result;
use actix_web::Scope;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::exemplar::CounterWithExemplar;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;
use std::sync::Arc;

pub fn metrics_scope() -> Scope {
    web::scope("/metrics").service(web::resource("").route(web::get().to(metrics_handler)))
}

async fn metrics_handler(reg: web::Data<Arc<Registry>>) -> Result<HttpResponse> {
    let mut body = String::new();
    encode(&mut body, &reg).map_err(|e| {
        tracing::error!("Failed to encode metrics: {:?}", e);
        actix_web::error::ErrorInternalServerError(e)
    })?;
    Ok(
        HttpResponse::Ok()
            .content_type("application/openmetrics-text; version=1.0.0; charset=utf-8")
            .body(body),
    )
}

