use std::path::Path;
use std::sync::{Arc, Mutex};
use std::{fs::create_dir_all, path::PathBuf};

use anyhow::{bail, Context, Result};
use log::{error, warn};
use std::sync::mpsc::channel;

use pueue_lib::error::Error;
use pueue_lib::network::certificate::create_certificates;
use pueue_lib::network::message::Shutdown;
use pueue_lib::network::protocol::socket_cleanup;
use pueue_lib::network::secret::init_shared_secret;
use pueue_lib::settings::Settings;
use pueue_lib::state::State;

use self::state_helper::{restore_state, save_state};
use crate::daemon::network::socket::accept_incoming;
use crate::daemon::task_handler::{TaskHandler, TaskSender};

pub mod cli;
mod network;
mod pid;
/// Contains re-usable helper functions, that operate on the pueue-lib state.
pub mod state_helper;
mod task_handler;

/// The main entry point for the daemon logic.
/// It's basically the `main`, but publicly exported as a library.
/// That way we can properly do integration testing for the daemon.
///
/// For the purpose of testing, some things shouldn't be run during tests.
/// There are some global operations that crash during tests, such as the ctlc handler.
/// This is due to the fact, that tests in the same file are executed in multiple threads.
/// Since the threads own the same global space, this would crash.
pub async fn run(config_path: Option<PathBuf>, profile: Option<String>, test: bool) -> Result<()> {
    // Try to read settings from the configuration file.
    let (mut settings, config_found) =
        Settings::read(&config_path).context("Error while reading configuration.")?;

    // We couldn't find a configuration file.
    // This probably means that Pueue has been started for the first time and we have to create a
    // default config file once.
    if !config_found {
        if let Err(error) = settings.save(&config_path) {
            bail!("Failed saving config file: {error:?}.");
        }
    };

    // Load any requested profile.
    if let Some(profile) = &profile {
        settings.load_profile(profile)?;
    }

    #[allow(deprecated)]
    if settings.daemon.groups.is_some() {
        error!(
            "Please delete the 'daemon.groups' section from your config file.
            Run `pueue -vv` to see where your config is located.\n\
            It is no longer used and groups can now only be edited via the commandline interface. \n\n\
            Attention: The first time the daemon is restarted this update, the amount of parallel tasks per group will be reset to 1!!"
        )
    }

    init_directories(&settings.shared.pueue_directory())?;
    if !settings.shared.daemon_key().exists() && !settings.shared.daemon_cert().exists() {
        create_certificates(&settings.shared).context("Failed to create certificates.")?;
    }
    init_shared_secret(&settings.shared.shared_secret_path())
        .context("Failed to initialize shared secret.")?;
    pid::create_pid_file(&settings.shared.pid_path()).context("Failed to create pid file.")?;

    // Restore the previous state and save any changes that might have happened during this
    // process. If no previous state exists, just create a new one.
    // Create a new empty state if any errors occur, but print the error message.
    let state = match restore_state(&settings.shared.pueue_directory()) {
        Ok(Some(state)) => state,
        Ok(None) => State::new(),
        Err(error) => {
            warn!("Failed to restore previous state:\n {error:?}");
            warn!("Using clean state instead.");
            State::new()
        }
    };

    // Save the state once at the very beginning.
    save_state(&state, &settings).context("Failed to save state on startup.")?;
    let state = Arc::new(Mutex::new(state));

    let (sender, receiver) = channel();
    let sender = TaskSender::new(sender);
    let mut task_handler = TaskHandler::new(state.clone(), settings.clone(), receiver);

    // Don't set ctrlc and panic handlers during testing.
    // This is necessary for multithreaded integration testing, since multiple listener per process
    // aren't allowed. On top of this, ctrlc also somehow breaks test error output.
    if !test {
        setup_signal_panic_handling(&settings, &sender)?;
    }

    std::thread::spawn(move || {
        task_handler.run();
    });

    accept_incoming(sender, state.clone(), settings.clone()).await?;

    Ok(())
}

/// Initialize all directories needed for normal operation.
fn init_directories(pueue_dir: &Path) -> Result<()> {
    // Pueue base path
    if !pueue_dir.exists() {
        create_dir_all(pueue_dir).map_err(|err| {
            Error::IoPathError(pueue_dir.to_path_buf(), "creating main directory", err)
        })?;
    }

    // Task log dir
    let log_dir = pueue_dir.join("log");
    if !log_dir.exists() {
        create_dir_all(&log_dir)
            .map_err(|err| Error::IoPathError(log_dir, "creating log directory", err))?;
    }

    // Task certs dir
    let certs_dir = pueue_dir.join("certs");
    if !certs_dir.exists() {
        create_dir_all(&certs_dir)
            .map_err(|err| Error::IoPathError(certs_dir, "creating certificate directory", err))?;
    }

    // Task log dir
    let logs_dir = pueue_dir.join("task_logs");
    if !logs_dir.exists() {
        create_dir_all(&logs_dir)
            .map_err(|err| Error::IoPathError(logs_dir, "creating task log directory", err))?;
    }

    Ok(())
}

/// Setup signal handling and panic handling.
///
/// On SIGINT and SIGTERM, we exit gracefully by sending a DaemonShutdown message to the
/// TaskHandler. This is to prevent dangling processes and other weird edge-cases.
///
/// On panic, we want to cleanup existing unix sockets and the PID file.
fn setup_signal_panic_handling(settings: &Settings, sender: &TaskSender) -> Result<()> {
    let sender_clone = sender.clone();

    // This section handles Shutdown via SigTerm/SigInt process signals
    // Notify the TaskHandler, so it can shutdown gracefully.
    // The actual program exit will be done via the TaskHandler.
    ctrlc::set_handler(move || {
        // Notify the task handler
        sender_clone
            .send(Shutdown::Emergency)
            .expect("Failed to send Message to TaskHandler on Shutdown");
    })?;

    // Try to do some final cleanup, even if we panic.
    let settings_clone = settings.clone();
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // invoke the default handler and exit the process
        orig_hook(panic_info);

        // Cleanup the pid file
        if let Err(error) = pid::cleanup_pid_file(&settings_clone.shared.pid_path()) {
            println!("Failed to cleanup pid after panic.");
            println!("{error}");
        }

        // Remove the unix socket.
        if let Err(error) = socket_cleanup(&settings_clone.shared) {
            println!("Failed to cleanup socket after panic.");
            println!("{error}");
        }

        std::process::exit(1);
    }));

    Ok(())
}
