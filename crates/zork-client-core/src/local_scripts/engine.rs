use super::*;
use rquickjs::{CatchResultExt, Context as JsContext, Function, Module, Runtime};
use std::sync::atomic::AtomicU64;

pub(super) fn execute(
    controller: Arc<Controller>,
    id: &str,
    source: &str,
    cancelled: Arc<AtomicBool>,
    cancel_wake: Arc<tokio::sync::Notify>,
    deadline: Instant,
    executor: &tokio::runtime::Handle,
) -> Result<()> {
    let runtime = Runtime::new()?;
    runtime.set_memory_limit(16 * 1024 * 1024);
    runtime.set_max_stack_size(512 * 1024);
    let started = Instant::now();
    let waiting = Arc::new(AtomicU64::new(0));
    let wait_time = waiting.clone();
    let stop = cancelled.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        stop.load(Ordering::Relaxed)
            || Instant::now() >= deadline
            || started
                .elapsed()
                .saturating_sub(Duration::from_micros(wait_time.load(Ordering::Relaxed)))
                >= Duration::from_secs(5)
    })));
    let context = JsContext::full(&runtime)?;
    context.with(|ctx| -> Result<()> {
        let owner = controller.clone();
        let run_id = id.to_owned();
        let executor = executor.clone();
        let call = Function::new(ctx.clone(), move |input: String| -> String {
            let outcome = (|| -> Result<Value> {
                ensure!(
                    input.len() <= operations::MAX_DATA * 2,
                    "Android call too large"
                );
                let value: Value = serde_json::from_str(&input)?;
                let started = Instant::now();
                let result = if value["method"] == "sleep" {
                    let ms = value["milliseconds"]
                        .as_u64()
                        .context("Invalid sleep duration")?;
                    ensure!(ms <= 30_000, "Sleep is limited to 30 seconds");
                    executor.block_on(async {
                        let wake = cancel_wake.notified();
                        tokio::pin!(wake);
                        wake.as_mut().enable();
                        ensure!(
                            !cancelled.load(Ordering::Relaxed) && Instant::now() < deadline,
                            "Script cancelled or timed out"
                        );
                        let end = (Instant::now() + Duration::from_millis(ms)).min(deadline);
                        tokio::select! {
                            _ = wake => anyhow::bail!("Script cancelled"),
                            _ = tokio::time::sleep_until(end.into()) => {
                                ensure!(Instant::now() < deadline, "Script timed out");
                                Ok(Value::Null)
                            }
                        }
                    })
                } else {
                    owner.call(&run_id, serde_json::from_value(value)?, &executor)
                };
                waiting.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                result
            })();
            match outcome {
                Ok(value) => json!({"value":value}),
                Err(error) => json!({"error":error.to_string()}),
            }
            .to_string()
        })?;
        ctx.globals().set("__androidCall", call)?;
        let owner = controller.clone();
        let run_id = id.to_owned();
        ctx.globals().set(
            "__localLog",
            Function::new(ctx.clone(), move |text: String| -> String {
                owner
                    .log(&run_id, &text)
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            })?,
        )?;
        ctx.eval::<(), _>(include_str!("prelude.js"))
            .catch(&ctx)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Module::evaluate(ctx.clone(), "chat-local-script.mjs", source)
            .and_then(|promise| promise.finish::<()>())
            .catch(&ctx)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(())
    })
}
