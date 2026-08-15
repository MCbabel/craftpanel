use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::auth::error::{Failure, Result};
use crate::auth::{extract, Admin, Disks, JsonBody, LiveServers, Params};
use crate::model::{AuthOptions, Id, PanelUser, RegistrationList, Timestamp};
use crate::registration::{Registrations, Verified};
use crate::AppState;

const PAGE: u32 = 50;
const PAGE_CEILING: u32 = 200;

pub fn router(service: Arc<Registrations>) -> Router<AppState> {
    with_live(service, LiveServers::none(), Disks::none())
}

pub fn with_live(
    service: Arc<Registrations>,
    live: LiveServers,
    disks: Disks,
) -> Router<AppState> {
    Router::new()
        .route("/auth/options", get(options))
        .route("/auth/register", post(register))
        .route("/auth/verify-email", post(verify_email))
        .route("/auth/verify-email/resend", post(resend))
        .route("/admin/registrations", get(queue))
        .route("/admin/registrations/{id}/approve", post(approve))
        .route("/admin/registrations/{id}/reject", post(reject))
        .layer(Extension(service))
        .layer(Extension(live))
        .layer(Extension(disks))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

#[derive(Deserialize)]
struct RegisterRequest {
    username: String,
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct TokenRequest {
    token: String,
}

#[derive(Deserialize)]
struct AddressRequest {
    email: String,
}

#[derive(Deserialize)]
struct RejectRequest {
    reason: Option<String>,
}

#[derive(Deserialize)]
struct PageQuery {
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Serialize)]
struct Accepted {
    status: &'static str,
}

#[derive(Serialize)]
struct VerifiedResponse {
    state: Verified,
}

async fn options(Extension(service): Extension<Arc<Registrations>>) -> Result<Json<AuthOptions>> {
    Ok(Json(service.options().await?))
}

async fn register(
    Extension(service): Extension<Arc<Registrations>>,
    parts: Parts,
    JsonBody(body): JsonBody<RegisterRequest>,
) -> Result<(StatusCode, Json<Accepted>)> {
    service
        .apply(
            &body.username,
            &body.email,
            &body.password,
            caller_address(&parts),
            Timestamp::now(),
        )
        .await?;
    Ok(accepted())
}

async fn verify_email(
    Extension(service): Extension<Arc<Registrations>>,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    parts: Parts,
    JsonBody(body): JsonBody<TokenRequest>,
) -> Result<Json<VerifiedResponse>> {
    let state = service
        .verify(&body.token, caller_address(&parts), &live, &disks, Timestamp::now())
        .await?;
    Ok(Json(VerifiedResponse { state }))
}

async fn resend(
    Extension(service): Extension<Arc<Registrations>>,
    JsonBody(body): JsonBody<AddressRequest>,
) -> Result<(StatusCode, Json<Accepted>)> {
    service.resend(&body.email, Timestamp::now()).await?;
    Ok(accepted())
}

fn accepted() -> (StatusCode, Json<Accepted>) {
    (StatusCode::ACCEPTED, Json(Accepted { status: "check_your_email" }))
}

async fn queue(
    Admin(_): Admin,
    Extension(service): Extension<Arc<Registrations>>,
    Params(query): Params<PageQuery>,
) -> Result<Json<RegistrationList>> {
    let limit = query.limit.unwrap_or(PAGE).clamp(1, PAGE_CEILING);
    Ok(Json(service.queue(limit, query.offset.unwrap_or(0)).await?))
}

async fn approve(
    Admin(_): Admin,
    Extension(service): Extension<Arc<Registrations>>,
    Extension(live): Extension<LiveServers>,
    Extension(disks): Extension<Disks>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<PanelUser>)> {
    let user = service.approve(application(&id)?, &live, &disks).await?;
    Ok((StatusCode::CREATED, Json(user)))
}

async fn reject(
    Admin(_): Admin,
    Extension(service): Extension<Arc<Registrations>>,
    Path(id): Path<String>,
    body: Option<JsonBody<RejectRequest>>,
) -> Result<StatusCode> {
    let reason = body.and_then(|JsonBody(body)| body.reason);
    service.reject(application(&id)?, reason.as_deref(), Timestamp::now()).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn application(raw: &str) -> Result<Id> {
    raw.parse().map_err(|_| {
        Failure::not_found("registration_not_found", "no such application")
    })
}

fn caller_address(parts: &Parts) -> Option<IpAddr> {
    parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip())
}
