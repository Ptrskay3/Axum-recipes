use axum1::{
    cli::cli_manager,
    config::get_config,
    queue::run_worker_until_stopped,
    search::run_meili_indexer_until_stopped,
    startup::application,
    task::supervised_task,
    utils::{init_tracing_panic_hook, report_exit},
};
use tokio::sync::watch;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let initial_configuration = get_config().expect("Failed to read configuration.");
    let (tx, rx) = watch::channel(initial_configuration);
    let cfg = tx.borrow();
    let _guard = sentry::init((
        cfg.clone().sentry_dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
            ..Default::default()
        },
    ));
    drop(cfg);

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "axum1=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(sentry_tracing::layer())
        .init();

    init_tracing_panic_hook();

    let application_task = tokio::spawn(application(rx.clone()));
    let worker_task = run_worker_until_stopped(rx.clone());
    let meili_indexing_task = run_meili_indexer_until_stopped(rx.clone());

    let (meili_task_spawned, meili_supervisor) = supervised_task(meili_indexing_task);
    let (worker_task_spawned, worker_supervisor) = supervised_task(worker_task);

    let cli_manager_task =
        tokio::spawn(cli_manager(tx.clone(), meili_supervisor, worker_supervisor));

    tokio::select! {
        f = application_task => report_exit("server", f),
        f = meili_task_spawned => report_exit("meili indexing", f),
        f = worker_task_spawned => report_exit("queue", f),
        f = cli_manager_task => report_exit("CLI Manager", f),
    };

    Ok(())
}
