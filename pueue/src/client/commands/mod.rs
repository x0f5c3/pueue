//! This module contains the logic for all non-trivial commands, such as `follow`, `restart`,
//! `wait`, etc.
//!
//! "non-trivial" vaguely means that we, for instance, have to do additional requests to the
//! daemon, open some files on the filesystem, edit files and so on.
//! All commands that cannot be simply handled by handling requests or using `pueue_lib`.
use anyhow::Result;

use pueue_lib::network::message::Message;
use pueue_lib::network::protocol::*;
use pueue_lib::state::State;

mod edit;
mod format_state;
mod local_follow;
mod restart;
mod wait;

pub use edit::edit;
pub use format_state::format_state;
pub use local_follow::local_follow;
pub use restart::restart;
pub use wait::{wait, WaitTargetStatus};

// This is a helper function for easy retrieval of the current daemon state.
// The current daemon state is often needed in more complex commands.
pub async fn get_state(stream: &mut GenericStream) -> Result<State> {
    // Create the message payload and send it to the daemon.
    send_message(Message::Status, stream).await?;

    // Check if we can receive the response from the daemon
    let message = receive_message(stream).await?;

    match message {
        Message::StatusResponse(state) => Ok(*state),
        _ => unreachable!(),
    }
}
