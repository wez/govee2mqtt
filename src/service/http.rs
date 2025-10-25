use crate::service::coordinator::Coordinator;
use crate::service::device::{Device, DeviceState};
use crate::service::state::StateHandle;
use anyhow::Context;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::net::IpAddr;
use tower_http::services::ServeDir;

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

async fn resolve_device_for_control(
    state: &StateHandle,
    id: &str,
) -> Result<Coordinator, Response> {
    state
        .resolve_device_for_control(&id)
        .await
        .map_err(not_found)
}

async fn resolve_device_read_only(state: &StateHandle, id: &str) -> Result<Device, Response> {
    state.resolve_device_read_only(&id).await.map_err(not_found)
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
        pub state: Option<DeviceState>,
    }

    let devices: Vec<_> = devices
        .into_iter()
        .map(|d| DeviceItem {
            name: d.name(),
            room: d.room_name().map(|r| r.to_string()),
            ip: d.ip_addr(),
            state: d.device_state(),
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
    let device = resolve_device_for_control(&state, &id).await?;

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
    let device = resolve_device_for_control(&state, &id).await?;

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
    let device = resolve_device_for_control(&state, &id).await?;

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
    let device = resolve_device_for_control(&state, &id).await?;

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

    let device = resolve_device_for_control(&state, &id).await?;

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
    let device = resolve_device_for_control(&state, &id).await?;

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
    let device = resolve_device_read_only(&state, &id).await?;

    let scenes = state.device_list_scenes(&device).await.map_err(generic)?;

    Ok(Json(scenes).into_response())
}

async fn list_one_clicks(State(state): State<StateHandle>) -> Result<Response, Response> {
    let undoc = state
        .get_undoc_client()
        .await
        .ok_or_else(|| anyhow::anyhow!("Undoc API client is not available"))
        .map_err(generic)?;
    let items = undoc.parse_one_clicks().await.map_err(generic)?;

    Ok(Json(items).into_response())
}

async fn activate_one_click(
    State(state): State<StateHandle>,
    Path(name): Path<String>,
) -> Result<Response, Response> {
    let undoc = state
        .get_undoc_client()
        .await
        .ok_or_else(|| anyhow::anyhow!("Undoc API client is not available"))
        .map_err(generic)?;
    let items = undoc.parse_one_clicks().await.map_err(generic)?;
    let item = items
        .iter()
        .find(|item| item.name == name)
        .ok_or_else(|| anyhow::anyhow!("didn't find item {name}"))
        .map_err(not_found)?;

    let iot = state
        .get_iot_client()
        .await
        .ok_or_else(|| anyhow::anyhow!("AWS IoT client is not available"))
        .map_err(generic)?;

    iot.activate_one_click(&item).await.map_err(generic)?;

    Ok(response_with_code(StatusCode::OK, "ok"))
}

async fn redirect_to_index() -> Response {
    axum::response::Redirect::to("/assets/index.html").into_response()
}

fn build_router(state: StateHandle) -> Router {
    Router::new()
        .route("/api/devices", get(list_devices))
        .route("/api/device/{id}/power/on", get(device_power_on))
        .route("/api/device/{id}/power/off", get(device_power_off))
        .route(
            "/api/device/{id}/brightness/{level}",
            get(device_set_brightness),
        )
        .route(
            "/api/device/{id}/colortemp/{kelvin}",
            get(device_set_color_temperature),
        )
        .route("/api/device/{id}/color/{color}", get(device_set_color))
        .route("/api/device/{id}/scene/{scene}", get(device_set_scene))
        .route("/api/device/{id}/scenes", get(device_list_scenes))
        .route("/api/oneclicks", get(list_one_clicks))
        .route("/api/oneclick/activate/{scene}", get(activate_one_click))
        .route("/", get(redirect_to_index))
        .nest_service("/assets", ServeDir::new("assets"))
        .with_state(state)
}

#[cfg(test)]
#[test]
fn test_build_router() {
    // axum has a history of chaning the URL syntax across
    // semver bumps; while that is OK, the syntax changes
    // are not caught at compile time, so we need a runtime
    // check to verify that the syntax is still good.
    // This next line will panic if axum decides that
    // the syntax is bad.
    let _ = build_router(StateHandle::default());
}

pub async fn run_http_server(state: StateHandle, port: u16) -> anyhow::Result<()> {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .with_context(|| format!("run_http_server: binding to port {port}"))?;
    let addr = listener.local_addr()?;
    log::info!("http server addr is {addr:?}");
    if let Err(err) = axum::serve(listener, app).await {
        log::error!("http server stopped: {err:#}");
    }

    Ok(())
}
