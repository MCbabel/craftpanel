use std::sync::Arc;
use std::time::Duration;

use crate::model::{Id, OperationError, OperationErrorStep, OperationPhase};
use crate::ops::{Operations, Step};

use super::{Installed, JavaError, Progress, Result, Runtimes};

const TICK: Duration = Duration::from_millis(300);

pub async fn lay_out(
    runtimes: &Runtimes,
    operations: &Arc<Operations>,
    operation: Id,
    major: u32,
    span: (f64, f64),
) -> Result<Installed> {
    let progress = runtimes.watch(major);
    announce(operations, operation, span.0).await;

    let ticker = follow(operations, operation, Arc::clone(&progress), span);
    let laid = runtimes.install(major).await;
    ticker.abort();
    laid
}

pub fn blame(error: &JavaError) -> OperationError {
    OperationError {
        code: error.code().to_owned(),
        message: error.to_string(),
        step: match error {
            JavaError::Unreachable { .. } | JavaError::Exposed { .. } | JavaError::Write { .. } => {
                OperationErrorStep::Filesystem
            }
            JavaError::Interrupted { .. } => OperationErrorStep::Internal,
            _ => OperationErrorStep::Download,
        },
    }
}

async fn announce(operations: &Arc<Operations>, operation: Id, floor: f64) {
    let step = Step {
        phase: Some(OperationPhase::InstallingJava),
        progress: Some(floor),
        bytes_processed: Some(0),
        ..Step::default()
    };
    if let Err(fault) = operations.advance(operation, step).await {
        tracing::warn!("the Java step went missing: {}", fault.message());
    }
}

fn follow(
    operations: &Arc<Operations>,
    operation: Id,
    progress: Arc<Progress>,
    span: (f64, f64),
) -> tokio::task::JoinHandle<()> {
    let operations = Arc::clone(operations);
    let (floor, ceiling) = span;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            let step = Step {
                progress: Some(floor + progress.share() * (ceiling - floor)),
                bytes_processed: Some(progress.done()),
                ..Step::default()
            };
            if operations.advance(operation, step).await.is_err() {
                return;
            }
        }
    })
}
