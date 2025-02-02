use jni::{
	objects::{GlobalRef, JClass, JObject, JString, JValue},
	sys::jint,
	JNIEnv, JavaVM,
};
use log;
use std::{
	panic::{catch_unwind, AssertUnwindSafe},
	process::abort,
};

#[derive(Clone, Copy, Debug)]
enum SLF4JLogLevel {
	Trace = 0,
	Debug = 10,
	Info = 20,
	Warn = 30,
	Error = 40,
}

impl From<log::Level> for SLF4JLogLevel {
	fn from(level: log::Level) -> Self {
		use log::Level::*;
		match level {
			Error => Self::Error,
			Warn => Self::Warn,
			Info => Self::Info,
			Debug => Self::Debug,
			Trace => Self::Trace,
		}
	}
}

impl From<SLF4JLogLevel> for log::Level {
	fn from(level: SLF4JLogLevel) -> Self {
		use SLF4JLogLevel::*;
		match level {
			Error => Self::Error,
			Warn => Self::Warn,
			Info => Self::Info,
			Debug => Self::Debug,
			Trace => Self::Trace,
		}
	}
}

impl TryFrom<jint> for SLF4JLogLevel {
	type Error = ();

	fn try_from(level: jint) -> Result<Self, ()> {
		match level {
			0 => Ok(Self::Trace),
			10 => Ok(Self::Debug),
			20 => Ok(Self::Info),
			30 => Ok(Self::Warn),
			40 => Ok(Self::Error),
			_ => Err(()),
		}
	}
}

impl From<SLF4JLogLevel> for &str {
	fn from(level: SLF4JLogLevel) -> Self {
		use SLF4JLogLevel::*;
		match level {
			Trace => "trace",
			Debug => "debug",
			Info => "info",
			Warn => "warn",
			Error => "error",
		}
	}
}

struct SLF4JLogger {
	vm: JavaVM,
	logger_class: GlobalRef,
}

impl SLF4JLogger {
	fn new(env: JNIEnv, logger_obj: JObject) -> jni::errors::Result<Self> {
		Ok(Self { vm: env.get_java_vm()?, logger_class: env.new_global_ref(logger_obj)? })
	}

	fn log_impl(&self, record: &log::Record) -> jni::errors::Result<()> {
		let mut env = self.vm.get_env()?;
		let level: &str = SLF4JLogLevel::from(record.level()).into();
		let message = match record.level() {
			log::Level::Error =>
				if let Some(file) = record.file() {
					if let Some(line) = record.line() {
						format!("{}:{}: {}", file, line, record.args())
					} else {
						format!("{}: {}", file, record.args())
					}
				} else {
					format!("{}", record.args())
				},
			_ => format!("{}", record.args()),
		};

		const SIGNATURE: &str = "(Ljava/lang/String;)V";
		let jstr = env.new_string(message.clone())?;
		let jobj = JObject::from(jstr);
		let jvalue = JValue::Object(&jobj);
		let args = [jvalue];

		let result = env.call_method(&self.logger_class, level, SIGNATURE, args.as_slice());

		let throwable = env.exception_occurred()?;
		if **throwable == *JObject::null() {
			match result {
				Ok(r) => Ok(r),
				Err(e) => {
					eprintln!("Error logging to JVM: {:?}\n{}", e, message);
					Err(e)
				},
			}?;
		} else {
			eprintln!("Exception occurred logging to JVM:\n{}", message);
			env.exception_clear()?;
		}
		Ok(())
	}
}

/// Implement the Log trait for SLF4JLogger.
impl log::Log for SLF4JLogger {
	fn enabled(&self, _metadata: &log::Metadata) -> bool {
		true
	}

	/// If a logging attempt produces an error, ignore,
	/// since obviously we can't log it :-)
	fn log(&self, record: &log::Record) {
		if self.log_impl(record).is_err() {}
	}

	fn flush(&self) {}
}

/// This is important for logging failures. This should *not* be used normally because we don't want to crash the app!
fn abort_on_panic(f: impl FnOnce()) {
	catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|e| {
		let msg = {
			if let Some(msg) = e.downcast_ref::<&str>() {
				msg.to_string()
			} else if let Some(msg) = e.downcast_ref::<String>() {
				msg.to_string()
			} else {
				"unknown panic from native code".to_string()
			}
		};
		eprintln!("fatal error: {}", msg);
		abort();
	});
}

fn set_max_level_from_slf4j_level(max_level: jint) {
	let level: SLF4JLogLevel = max_level.try_into().unwrap_or(SLF4JLogLevel::Warn); // Default to warning if invalid log level passed

	log::set_max_level(log::Level::from(level).to_level_filter());
}

#[no_mangle]
pub unsafe extern "C" fn Java_io_amplica_graphsdk_Native_loggerInitialize(
	env: JNIEnv,
	_class: JClass,
	max_level: jint,
	logger_obj: JObject,
) {
	abort_on_panic(|| {
		let logger = SLF4JLogger::new(env, logger_obj).expect("could not initialize logging");

		match log::set_boxed_logger(Box::new(logger)) {
			Ok(_) => {
				set_max_level_from_slf4j_level(max_level);
				let backtrace_mode = {
					cfg_if::cfg_if! {
						if #[cfg(target_os = "android")] {
							log_panics::BacktraceMode::Unresolved
						} else {
							log_panics::BacktraceMode::Resolved
						}
					}
				};
				log_panics::Config::new().backtrace_mode(backtrace_mode).install_panic_hook();
				log::info!(
					"Initializing {} version:{}",
					env!("CARGO_PKG_NAME"),
					env!("CARGO_PKG_VERSION")
				);
			},
			Err(_) => {
				log::warn!(
					"Duplicate logger initialization ignored for {}",
					env!("CARGO_PKG_NAME")
				);
			},
		}
	});
}

#[no_mangle]
pub unsafe extern "C" fn Java_io_amplica_graphsdk_Native_loggerSetMaxLevel(
	_env: JNIEnv,
	_class: JClass,
	max_level: jint,
) {
	set_max_level_from_slf4j_level(max_level);
}

/// Function mainly just for testing the Java side of this implementation.
/// Can be called in production code, but there's really no reason to.
#[no_mangle]
pub unsafe extern "C" fn Java_io_amplica_graphsdk_LibraryTest_log(
	mut env: JNIEnv,
	_class: JClass,
	level: jint,
	message: JString,
) {
	let level: SLF4JLogLevel = match level.try_into() {
		Ok(level) => level,
		_ => SLF4JLogLevel::Info,
	};

	if let Ok(message_str) = env.get_string(&message) {
		let rust_str: String = message_str.into();
		log::log!(level.into(), "{}", rust_str);
	}
}
