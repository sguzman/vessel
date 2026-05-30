use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;

use vessel_core::{LoggingFormat, Result, VesselError};

static PROGRESS_ENABLED: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, Copy)]
pub struct LoggingOptions {
    pub quiet: bool,
    pub verbose: u8,
    pub progress: bool,
}

#[derive(Default)]
struct HumanEventFormatter;

#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    fields: Vec<(String, String)>,
}

pub fn init(level: &str, format: LoggingFormat, options: LoggingOptions) -> Result<()> {
    PROGRESS_ENABLED.store(options.progress, Ordering::Relaxed);
    let filter = EnvFilter::try_new(resolve_level(level, options))
        .map_err(|err| VesselError::Config(err.to_string()))?;

    match format {
        LoggingFormat::Human => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(false)
            .without_time()
            .event_format(HumanEventFormatter)
            .try_init(),
        LoggingFormat::Json => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .json()
            .with_target(true)
            .try_init(),
    }
    .map_err(|err| VesselError::Config(format!("logging init failed: {err}")))?;

    Ok(())
}

pub fn progress(target: &str, message: impl Into<String>) {
    if !PROGRESS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    tracing::info!(target: "progress", area = target, progress = true, "{}", message.into());
}

pub fn progress_enabled() -> bool {
    PROGRESS_ENABLED.load(Ordering::Relaxed)
}

fn resolve_level(configured: &str, options: LoggingOptions) -> String {
    if options.quiet {
        return "warn".to_owned();
    }

    match options.verbose {
        0 => configured.to_owned(),
        1 => "debug".to_owned(),
        _ => "trace".to_owned(),
    }
}

impl<S, N> FormatEvent<S, N> for HumanEventFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        write!(writer, "[{}]", level_label(meta.level()))?;
        let target = meta.target();
        let display_target = visitor
            .fields
            .iter()
            .position(|(key, _)| key == "area")
            .map(|index| visitor.fields.remove(index).1)
            .unwrap_or_else(|| display_target(target));
        if !display_target.is_empty()
            && display_target != "vessel"
            && display_target != level_label(meta.level())
        {
            write!(writer, " [{}]", display_target)?;
        }
        if let Some(message) = visitor.message {
            write!(writer, " {message}")?;
        } else {
            ctx.field_format().format_fields(writer.by_ref(), event)?;
        }
        for (key, value) in visitor.fields {
            write!(writer, " {key}={value}")?;
        }
        writeln!(writer)
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(trim_quotes(rendered));
        } else if field.name() != "progress" {
            self.fields.push((field.name().to_owned(), trim_quotes(rendered)));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_owned());
        } else if field.name() != "progress" {
            self.fields.push((field.name().to_owned(), value.to_owned()));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() != "progress" {
            self.fields
                .push((field.name().to_owned(), value.to_string()));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }
}

fn level_label(level: &tracing::Level) -> &'static str {
    match *level {
        tracing::Level::ERROR => "error",
        tracing::Level::WARN => "warn",
        tracing::Level::INFO => "info",
        tracing::Level::DEBUG => "debug",
        tracing::Level::TRACE => "trace",
    }
}

fn display_target(target: &str) -> String {
    if target.contains("::") {
        target.rsplit("::").next().unwrap_or(target).to_owned()
    } else {
        target.to_owned()
    }
}

fn trim_quotes(value: String) -> String {
    value
        .strip_prefix('"')
        .and_then(|next| next.strip_suffix('"'))
        .unwrap_or(&value)
        .to_owned()
}
