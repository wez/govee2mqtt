use crate::service::device::Device;
use crate::service::state::StateHandle;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::net::IpAddr;
use uncased::Uncased;

fn response_with_code<T: ToString + std::fmt::Display>(code: StatusCode, err: T) -> Response {
    if !code.is_success() {
        log::error!("err: {err:#}");
    }

    let mut response = Json(serde_json::json!({
        "code": code.as_u16(),
        "msg": format!("{err:#}")
    }))
    .into_response();
    *response.status_mut() = code;
    response
}

fn generic<T: ToString + std::fmt::Display>(err: T) -> Response {
    response_with_code(StatusCode::INTERNAL_SERVER_ERROR, err)
}

fn not_found<T: ToString + std::fmt::Display>(err: T) -> Response {
    response_with_code(StatusCode::NOT_FOUND, err)
}

fn bad_request<T: ToString + std::fmt::Display>(err: T) -> Response {
    response_with_code(StatusCode::BAD_REQUEST, err)
}

async fn resolve_device(state: &StateHandle, id: &str) -> Result<Device, Response> {
    state
        .resolve_device(&id)
        .await
        .ok_or_else(|| not_found(format!("device '{id}' not found")))
}

/// Returns a json array of device information
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

/// Turns on a given device
async fn device_power_on(
    State(state): State<StateHandle>,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let device = resolve_device(&state, &id).await?;

    state
        .device_power_on(&device, true)
        .await
        .map_err(generic)?;

    Ok(response_with_code(StatusCode::OK, "ok"))
}

/// Turns off a given device
async fn device_power_off(
    State(state): State<StateHandle>,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let device = resolve_device(&state, &id).await?;

    state
        .device_power_on(&device, false)
        .await
        .map_err(generic)?;

    Ok(response_with_code(StatusCode::OK, "ok"))
}

/// Sets the brightness level of a given device
async fn device_set_brightness(
    State(state): State<StateHandle>,
    Path((id, level)): Path<(String, u8)>,
) -> Result<Response, Response> {
    let device = resolve_device(&state, &id).await?;

    state
        .device_set_brightness(&device, level)
        .await
        .map_err(generic)?;

    Ok(response_with_code(StatusCode::OK, "ok"))
}

/// Sets the color temperature of a given device
async fn device_set_color_temperature(
    State(state): State<StateHandle>,
    Path((id, kelvin)): Path<(String, u32)>,
) -> Result<Response, Response> {
    let device = resolve_device(&state, &id).await?;

    state
        .device_set_color_temperature(&device, kelvin)
        .await
        .map_err(generic)?;

    Ok(response_with_code(StatusCode::OK, "ok"))
}

/// Sets the RGB color of a given device
async fn device_set_color(
    State(state): State<StateHandle>,
    Path((id, color)): Path<(String, String)>,
) -> Result<Response, Response> {
    let color = csscolorparser::parse(&color)
        .map_err(|err| bad_request(format!("error parsing color '{color}': {err}")))?;
    let [r, g, b, _a] = color.to_rgba8();

    let device = resolve_device(&state, &id).await?;

    state
        .device_set_color_rgb(&device, r, g, b)
        .await
        .map_err(generic)?;

    Ok(response_with_code(StatusCode::OK, "ok"))
}

/// Activates the named scene for a given device
async fn device_set_scene(
    State(state): State<StateHandle>,
    Path((id, scene)): Path<(String, String)>,
) -> Result<Response, Response> {
    let device = resolve_device(&state, &id).await?;

    state
        .device_set_scene(&device, &scene)
        .await
        .map_err(generic)?;

    Ok(response_with_code(StatusCode::OK, "ok"))
}

/// Returns a JSON array of the available scene names for a given device
async fn device_list_scenes(
    State(state): State<StateHandle>,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let device = resolve_device(&state, &id).await?;

    let mut scenes: Vec<_> = state
        .device_list_scenes(&device)
        .await
        .map_err(generic)?
        .into_iter()
        .map(Uncased::new)
        .collect();
    scenes.sort();
    let scenes: Vec<_> = scenes.into_iter().map(|u| u.into_string()).collect();

    Ok(Json(scenes).into_response())
}

pub async fn run_http_server(state: StateHandle, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/api/devices", get(list_devices))
        .route("/api/device/:id/power/on", get(device_power_on))
        .route("/api/device/:id/power/off", get(device_power_off))
        .route(
            "/api/device/:id/brightness/:level",
            get(device_set_brightness),
        )
        .route(
            "/api/device/:id/colortemp/:kelvin",
            get(device_set_color_temperature),
        )
        .route("/api/device/:id/color/:color", get(device_set_color))
        .route("/api/device/:id/scene/:scene", get(device_set_scene))
        .route("/api/device/:id/scenes", get(device_list_scenes))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    let addr = listener.local_addr()?;
    log::info!("http server addr is {addr:?}");
    if let Err(err) = axum::serve(listener, app).await {
        log::error!("http server stopped: {err:#}");
    }

    Ok(())
}
