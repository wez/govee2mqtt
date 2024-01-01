use crate::service::state::StateHandle;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::net::IpAddr;

fn generic<T: ToString + std::fmt::Display>(err: T) -> Response {
    log::error!("err: {err:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
}

async fn list_devices(State(state): State<StateHandle>) -> Result<Response, Response> {
    let mut devices = state.devices().await;
    devices.sort_by_key(|d| (d.room_name().map(|name| name.to_string()), d.name()));

    #[derive(Serialize)]
    struct DeviceItem {
        pub sku: String,
        pub id: String,
        pub name: String,
        pub room: Option<String>,
        pub ip: Option<IpAddr>,
    }

    let devices: Vec<_> = devices
        .into_iter()
        .map(|d| DeviceItem {
            name: d.name(),
            room: d.room_name().map(|r| r.to_string()),
            ip: d.ip_addr(),
            sku: d.sku,
            id: d.id,
        })
        .collect();

    Ok(Json(devices).into_response())
}

pub async fn run_http_server(state: StateHandle, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/api/devices", get(list_devices))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    let addr = listener.local_addr()?;
    log::info!("http server addr is {addr:?}");
    if let Err(err) = axum::serve(listener, app).await {
        log::error!("http server stopped: {err:#}");
    }

    Ok(())
}
