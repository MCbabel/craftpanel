use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};

use crate::auth::error::{Failure, Result};
use crate::auth::{extract, Admin, LiveServers};
use crate::java::{Inventory, RuntimeOverview};
use crate::AppState;

pub fn router(inventory: Arc<Inventory>, live: LiveServers) -> Router<AppState> {
    Router::new()
        .route("/admin/java-runtimes", get(overview))
        .route("/admin/java-runtimes/{major}", post(fetch).delete(remove))
        .layer(Extension(inventory))
        .layer(Extension(live))
        .layer(axum::middleware::from_fn(extract::same_origin))
}

async fn overview(
    _: Admin,
    Extension(inventory): Extension<Arc<Inventory>>,
    Extension(live): Extension<LiveServers>,
) -> Result<Json<RuntimeOverview>> {
    Ok(Json(inventory.overview(&live).await?))
}

async fn fetch(
    _: Admin,
    Extension(inventory): Extension<Arc<Inventory>>,
    Extension(live): Extension<LiveServers>,
    Path(major): Path<String>,
) -> Result<(StatusCode, Json<RuntimeOverview>)> {
    inventory.start(asked(&major)?, &live).await?;
    Ok((StatusCode::ACCEPTED, Json(inventory.overview(&live).await?)))
}

async fn remove(
    _: Admin,
    Extension(inventory): Extension<Arc<Inventory>>,
    Extension(live): Extension<LiveServers>,
    Path(major): Path<String>,
) -> Result<Json<RuntimeOverview>> {
    inventory.remove(asked(&major)?, &live).await?;
    Ok(Json(inventory.overview(&live).await?))
}

fn asked(major: &str) -> Result<u32> {
    major.parse().map_err(|_| {
        Failure::not_found("java_major_unknown", format!("{major} is no Java version"))
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::harness::{
        a_user, an_admin, as_user, empty, fetch as get, sign_in, state_with, test_pool,
    };
    use crate::config::Config;
    use crate::java::harness::{a_data_dir, a_jre, FakeAdoptium, Scratch};
    use crate::java::Runtimes;

    struct World {
        app: axum::Router,
        dir: Scratch,
    }

    async fn a_world(upstream: Option<&FakeAdoptium>) -> (World, String) {
        let pool = test_pool().await;
        let dir = a_data_dir();
        let base = upstream.map_or("http://127.0.0.1:1", FakeAdoptium::base);
        let runtimes = Arc::new(Runtimes::with_base(dir.path(), base).expect("a client"));
        let inventory =
            Arc::new(Inventory::new(pool.clone(), runtimes, dir.path()));

        let config = Config { data_dir: dir.path().to_path_buf(), ..Config::default() };
        let app = router(inventory, LiveServers::none()).with_state(state_with(&pool, config));
        let secret = sign_in(&pool, an_admin(&pool, "boss").await).await;
        (World { app, dir }, secret)
    }

    async fn call(world: &World, request: axum::http::Request<axum::body::Body>) -> (StatusCode, Value) {
        let response = world.app.clone().oneshot(request).await.expect("a response");
        let status = response.status();
        (status, crate::auth::harness::body_json(response).await)
    }

    fn row(seen: &Value, major: u32) -> Value {
        seen["majors"]
            .as_array()
            .expect("the rows")
            .iter()
            .find(|entry| entry["major"] == major)
            .expect("a row for that major")
            .clone()
    }

    async fn settled(world: &World, secret: &str, major: u32) -> Value {
        for _ in 0..200 {
            let (_, seen) = call(world, as_user(get("/admin/java-runtimes"), secret)).await;
            let entry = row(&seen, major);
            if entry["job"].is_null() || entry["job"]["running"] == false {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the fetch never came to rest");
    }

    #[tokio::test]
    async fn only_an_administrator_sees_the_runtimes_or_touches_them() {
        let (world, _) = a_world(None).await;
        let pool_secret = {
            let pool = test_pool().await;
            sign_in(&pool, a_user(&pool, "max").await).await
        };

        let (status, _) = call(&world, get("/admin/java-runtimes")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "signed out sees nothing");

        let (status, _) =
            call(&world, as_user(get("/admin/java-runtimes"), &pool_secret)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "a session of another panel is no session");
    }

    #[tokio::test]
    async fn a_plain_user_is_turned_away_from_every_one_of_the_three() {
        let pool = test_pool().await;
        let dir = a_data_dir();
        let runtimes =
            Arc::new(Runtimes::with_base(dir.path(), "http://127.0.0.1:1").expect("a client"));
        let inventory = Arc::new(Inventory::new(pool.clone(), runtimes, dir.path()));
        let config = Config { data_dir: dir.path().to_path_buf(), ..Config::default() };
        let app = router(inventory, LiveServers::none()).with_state(state_with(&pool, config));
        let world = World { app, dir };
        let secret = sign_in(&pool, a_user(&pool, "max").await).await;

        for request in [
            as_user(get("/admin/java-runtimes"), &secret),
            as_user(empty("POST", "/admin/java-runtimes/21"), &secret),
            as_user(empty("DELETE", "/admin/java-runtimes/21"), &secret),
        ] {
            let (status, _) = call(&world, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn the_overview_carries_a_row_for_every_major_and_the_machine_it_runs_on() {
        let (world, secret) = a_world(None).await;

        let (status, seen) = call(&world, as_user(get("/admin/java-runtimes"), &secret)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(seen["auto_install"], true);
        assert_eq!(seen["majors"].as_array().expect("the rows").len(), 4);
        assert_eq!(row(&seen, 8)["fetchable"], true);
        assert!(row(&seen, 8)["runtime"].is_null());
        assert!(seen["directory"].as_str().expect("a path").ends_with("runtimes"));
    }

    #[tokio::test]
    async fn a_version_the_panel_does_not_fetch_is_answered_before_anything_is_started() {
        let (world, secret) = a_world(None).await;

        let (status, body) =
            call(&world, as_user(empty("POST", "/admin/java-runtimes/11"), &secret)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "java_major_unknown");

        let (status, body) =
            call(&world, as_user(empty("POST", "/admin/java-runtimes/nonsense"), &secret)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "java_major_unknown");
    }

    #[tokio::test]
    async fn one_press_lays_the_runtime_down_and_a_second_one_replaces_it_with_the_newer_build() {
        let upstream = FakeAdoptium::started().await;
        upstream.offer(21, "21.0.12+7", a_jre("21.0.12+7"));
        let (world, secret) = a_world(Some(&upstream)).await;

        let (status, _) =
            call(&world, as_user(empty("POST", "/admin/java-runtimes/21"), &secret)).await;
        assert_eq!(status, StatusCode::ACCEPTED);

        let seen = settled(&world, &secret, 21).await;
        let laid = row(&seen, 21);
        assert_eq!(laid["runtime"]["version"], "21.0.12+7");
        assert_eq!(laid["runtime"]["vendor"], "temurin");
        assert!(laid["runtime"]["size_bytes"].as_u64().expect("a size") > 0);
        assert!(world.dir.path().join("runtimes").join("java-21").is_dir());

        upstream.offer(21, "21.0.13+9", a_jre("21.0.13+9"));
        let (status, _) =
            call(&world, as_user(empty("POST", "/admin/java-runtimes/21"), &secret)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "a runtime that is there is fetched again");

        let after = settled(&world, &secret, 21).await;
        assert_eq!(
            row(&after, 21)["runtime"]["version"], "21.0.13+9",
            "the newer build stands there now"
        );
        assert_eq!(upstream.served(), 2);
    }

    #[tokio::test]
    async fn a_fetch_that_fails_leaves_its_reason_standing_where_the_page_reads_it() {
        let upstream = FakeAdoptium::started().await;
        let (world, secret) = a_world(Some(&upstream)).await;

        let (status, _) =
            call(&world, as_user(empty("POST", "/admin/java-runtimes/17"), &secret)).await;
        assert_eq!(status, StatusCode::ACCEPTED);

        let seen = settled(&world, &secret, 17).await;
        let entry = row(&seen, 17);
        assert!(entry["runtime"].is_null(), "nothing was laid down");
        assert_eq!(entry["job"]["failure_code"], "java_download_unavailable");
        assert!(entry["job"]["failure"].as_str().expect("a sentence").contains("Adoptium"));
    }

    #[tokio::test]
    async fn what_was_fetched_can_be_taken_away_again() {
        let upstream = FakeAdoptium::started().await;
        upstream.offer(8, "8.0.502+7", a_jre("8.0.502+7"));
        let (world, secret) = a_world(Some(&upstream)).await;

        call(&world, as_user(empty("POST", "/admin/java-runtimes/8"), &secret)).await;
        settled(&world, &secret, 8).await;

        let (status, seen) =
            call(&world, as_user(empty("DELETE", "/admin/java-runtimes/8"), &secret)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(row(&seen, 8)["runtime"].is_null(), "the answer already says it is gone");
        assert!(!world.dir.path().join("runtimes").join("java-8").exists());

        let (status, body) =
            call(&world, as_user(empty("DELETE", "/admin/java-runtimes/8"), &secret)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "java_runtime_not_here");
    }
}
