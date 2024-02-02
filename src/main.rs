use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::info;
use warp::Filter;

static FILE_NAME: &str = "data.json";
static INTERVAL: u64 = 5;
static TIMEOUT: u64 = 1;
static RETAIN_SECONDS: u64 = 60 * 60 * 24 * 7;
static INDEX_HTML: &str = include_str!("../index.html");

#[tokio::main]
async fn main() {
    setup_tracing();

    let data_route = warp::get()
        .and(warp::path("data"))
        .and_then(|| handle_get_data());

    let index_route = warp::get().and(warp::path::end()).and_then(|| handle_get_index());

    let server = warp::serve(data_route.or(index_route)).run(([0, 0, 0, 0], 5000));
    let monitor = run_monitor();

    tokio::select! {
        _ = server => {},
        _ = monitor => {}
    }
}

#[derive(Serialize, Deserialize)]
struct Data {
    states: Vec<StateTime>,
    last_update: u64,
}

#[derive(Serialize, Deserialize)]
struct StateTime {
    start: u64,
    state: State,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Debug)]
enum State {
    Unknown,
    Ok,
    Err,
}

async fn load_data() -> Result<Data, Box<dyn std::error::Error>> {
    if !tokio::fs::metadata(FILE_NAME).await.is_ok() {
        let data = Data {
            states: vec![],
            last_update: current_time(),
        };
        return Ok(data);
    }

    let file = tokio::fs::read_to_string(FILE_NAME).await?;
    let data: Data = serde_json::from_str(&file)?;
    Ok(data)
}

async fn save_data(data: &Data) -> Result<(), Box<dyn std::error::Error>> {
    let temp_file = format!("{}.tmp", FILE_NAME);
    let data = serde_json::to_string(&data)?;
    tokio::fs::write(&temp_file, data).await?;
    tokio::fs::rename(&temp_file, FILE_NAME).await?;
    Ok(())
}

fn current_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

async fn check_state(client: &Client) -> State {
    match client
        .get("http://ismycomputeron.com/")
        .timeout(Duration::from_secs(TIMEOUT))
        .send()
        .await
    {
        Ok(_) => State::Ok,
        Err(e) => {
            info!("Request failed: {}", e);
            State::Err
        }
    }
}

async fn run_monitor() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();

    let mut data = load_data().await?;

    // Pad the last state to the current time with unknown
    if let Some(last) = data.states.last() {
        if last.state != State::Unknown && last.start < data.last_update {
            info!(
                "Padding unknown from {} to {}",
                last.start, data.last_update
            );
            data.states.push(StateTime {
                start: data.last_update,
                state: State::Unknown,
            });
        }
    }

    let mut last_state = data
        .states
        .last()
        .map(|s| s.state.clone())
        .unwrap_or(State::Unknown);

    loop {
        let time = current_time();
        info!("Checking state at {}", time);
        let state = check_state(&client).await;
        if state != last_state {
            info!("State changed to {:?}", state);
            data.states.push(StateTime {
                start: time,
                state: state.clone(),
            });
            last_state = state;
        }
        data.last_update = time;
        // Make sure this doesn't grow forever
        data.states.retain(|s| s.start > time - RETAIN_SECONDS);
        save_data(&data).await?;
        tokio::time::sleep(Duration::from_secs(INTERVAL)).await;
    }
}

async fn handle_get_data() -> Result<impl warp::Reply, warp::Rejection> {
    info!("GET /data");
    let data = load_data().await.unwrap();
    Ok(warp::reply::json(&data))
}

fn setup_tracing() {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(false)
        .init();
}

async fn handle_get_index() -> Result<impl warp::Reply, warp::Rejection> {
    info!("GET /");
    Ok(warp::reply::html(INDEX_HTML))
}
