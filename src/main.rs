mod archive;
mod gcode;
mod printer;
mod updates;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::{net::TcpListener, sync::Semaphore, task::spawn_blocking};

#[derive(Parser)]
#[command(
    version,
    about = "Compare les tranchages Bambu Studio et envoie un plateau à une P2S."
)]
struct Args {
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long)]
    sans_navigateur: bool,
    /// Vérifier les publications GitHub depuis le raccourci installé sous Windows.
    #[arg(long, hide = true)]
    mises_a_jour: bool,
}

#[derive(Clone)]
struct App {
    address: String,
    analysis: Arc<Semaphore>,
    printing: Arc<Semaphore>,
    version: tokio::sync::watch::Receiver<updates::Status>,
}

#[derive(Deserialize)]
struct Upload {
    nom: String,
}

#[derive(Deserialize)]
struct PrintRequest {
    printer: printer::Credentials,
    member: String,
    digest: String,
    trays: Vec<u16>,
    confirmed: bool,
}

struct ApiError(StatusCode, String);

impl ApiError {
    fn bad(error: anyhow::Error) -> Self {
        Self(StatusCode::BAD_REQUEST, error.to_string())
    }

    fn printer(error: anyhow::Error) -> Self {
        Self(StatusCode::BAD_GATEWAY, format!("{error:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

fn authorize(app: &App, headers: &HeaderMap) -> ApiResult<()> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    if origin == Some(format!("http://{}", app.address).as_str()) {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Ouvre l’adresse locale de l’Atelier G-code.".into(),
        ))
    }
}

async fn local_only(State(app): State<App>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if host == Some(app.address.as_str()) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

async fn analyze(
    State(app): State<App>,
    headers: HeaderMap,
    Query(upload): Query<Upload>,
    body: Bytes,
) -> ApiResult<Json<Vec<archive::Report>>> {
    authorize(&app, &headers)?;
    let permit = app.analysis.try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::CONFLICT,
            "Une analyse est en cours. Réessaie après son résultat.".into(),
        )
    })?;
    let filename = upload
        .nom
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_owned();
    let reports = spawn_blocking(move || {
        let _permit = permit;
        archive::inspect(&filename, &body)
    })
    .await
    .context("L’analyse a été interrompue.")
    .map_err(ApiError::bad)?
    .map_err(ApiError::bad)?;
    Ok(Json(reports))
}

async fn connection(
    State(app): State<App>,
    headers: HeaderMap,
    Json(printer): Json<printer::Credentials>,
) -> ApiResult<Json<printer::PrinterStatus>> {
    authorize(&app, &headers)?;
    printer.validate().map_err(ApiError::bad)?;
    let status = spawn_blocking(move || printer::status(&printer))
        .await
        .context("La lecture de l’état a été interrompue.")
        .map_err(ApiError::printer)?
        .map_err(ApiError::printer)?;
    Ok(Json(status))
}

async fn print(
    State(app): State<App>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<printer::Dispatch>> {
    authorize(&app, &headers)?;
    let request: PrintRequest = serde_json::from_slice(
        headers
            .get("x-atelier-print")
            .map(HeaderValue::as_bytes)
            .unwrap_or_default(),
    )
    .context("La demande d’impression est invalide.")
    .map_err(ApiError::bad)?;
    if !request.confirmed {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Confirme le plateau et les filaments avant l’envoi.".into(),
        ));
    }
    request.printer.validate().map_err(ApiError::bad)?;
    let permit = app.printing.try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::CONFLICT,
            "Un envoi vers l’imprimante est déjà en cours.".into(),
        )
    })?;
    let dispatch = spawn_blocking(move || -> ApiResult<_> {
        let _permit = permit;
        let plate =
            archive::prepare(&body, &request.member, &request.digest).map_err(ApiError::bad)?;
        printer::start(&request.printer, &plate, &request.trays, &body).map_err(ApiError::printer)
    })
    .await
    .context("L’envoi a été interrompu. Vérifie l’écran de la P2S avant de réessayer.")
    .map_err(ApiError::printer)??;
    Ok(Json(dispatch))
}

fn asset(bytes: &'static [u8], content_type: &'static str) -> Response {
    ([
        (header::CONTENT_TYPE, content_type),
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src data:; base-uri 'none'; frame-ancestors 'none'"),
    ], bytes).into_response()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    #[cfg(windows)]
    let _session = updates::hold_installation()?;
    let listener = TcpListener::bind(("127.0.0.1", args.port))
        .await
        .context("Impossible d’ouvrir le port local de l’Atelier G-code.")?;
    let address = listener.local_addr()?.to_string();
    let app = App {
        address: address.clone(),
        analysis: Arc::new(Semaphore::new(1)),
        printing: Arc::new(Semaphore::new(1)),
        version: updates::start(args.mises_a_jour),
    };
    let router = Router::new()
        .route(
            "/version",
            get(|State(app): State<App>| async move { Json(app.version.borrow().clone()) }),
        )
        .route(
            "/",
            get(|| async {
                asset(
                    include_bytes!("../interface/index.html"),
                    "text/html; charset=utf-8",
                )
            }),
        )
        .route(
            "/styles.css",
            get(|| async {
                asset(
                    include_bytes!("../interface/styles.css"),
                    "text/css; charset=utf-8",
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                asset(
                    include_bytes!("../interface/app.js"),
                    "text/javascript; charset=utf-8",
                )
            }),
        )
        .route(
            "/print.js",
            get(|| async {
                asset(
                    include_bytes!("../interface/print.js"),
                    "text/javascript; charset=utf-8",
                )
            }),
        )
        .route(
            "/exemple.json",
            get(|| async {
                let mut reports = archive::inspect(
                    "Deux carrés (exemple synthétique).gcode",
                    include_bytes!("../interface/exemple.gcode"),
                )
                .map_err(ApiError::bad)?;
                reports
                    .pop()
                    .map(Json)
                    .context("Exemple illisible.")
                    .map_err(ApiError::bad)
            }),
        )
        .route(
            "/analyse",
            post(analyze).layer(DefaultBodyLimit::max(archive::MAX_BYTES)),
        )
        .route(
            "/connexion",
            post(connection).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/imprimer",
            post(print).layer(DefaultBodyLimit::max(archive::MAX_BYTES)),
        )
        .layer(middleware::from_fn_with_state(app.clone(), local_only))
        .with_state(app);
    let url = format!("http://{address}/");
    println!("Atelier G-code · {url}");
    println!("Ctrl+C pour quitter.");
    if !args.sans_navigateur {
        let _ = open::that_detached(url);
    }
    axum::serve(listener, router).await?;
    Ok(())
}
