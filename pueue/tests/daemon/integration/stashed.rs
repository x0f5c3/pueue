use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local};
use pueue_lib::state::GroupStatus;
use rstest::rstest;

use pueue_lib::network::message::*;
use pueue_lib::settings::Shared;
use pueue_lib::task::*;

use crate::helper::*;

/// Helper to pause the whole daemon
pub async fn add_stashed_task(
    shared: &Shared,
    command: &str,
    stashed: bool,
    enqueue_at: Option<DateTime<Local>>,
) -> Result<Message> {
    let mut message = create_add_message(shared, command);
    message.stashed = stashed;
    message.enqueue_at = enqueue_at;

    send_message(shared, message)
        .await
        .context("Failed to to add task message")
}

/// Tasks can be stashed and scheduled for being enqueued at a specific point in time.
///
/// Furthermore these stashed tasks can then be manually enqueued again.
#[rstest]
#[case(true, None)]
#[case(true, Some(Local::now() + Duration::minutes(2)))]
#[case(false, Some(Local::now() + Duration::minutes(2)))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_enqueued_tasks(
    #[case] stashed: bool,
    #[case] enqueue_at: Option<DateTime<Local>>,
) -> Result<()> {
    let daemon = daemon().await?;
    let shared = &daemon.settings.shared;

    assert_success(add_stashed_task(shared, "sleep 10", stashed, enqueue_at).await?);

    // The task should be added in stashed state.
    let task = wait_for_task_condition(shared, 0, |task| {
        matches!(task.status, TaskStatus::Stashed { .. })
    })
    .await?;

    assert!(
        task.enqueued_at.is_none(),
        "Enqueued tasks shouldn't have an enqeued_at date set."
    );

    // Assert the correct point in time has been set, in case `enqueue_at` is specific.
    if enqueue_at.is_some() {
        let status = get_task_status(shared, 0).await?;
        assert!(matches!(status, TaskStatus::Stashed { .. }));

        if let TaskStatus::Stashed { enqueue_at: inner } = status {
            assert_eq!(inner, enqueue_at);
        }
    }

    let pre_enqueue_time = Local::now();

    // Manually enqueue the task
    let enqueue_message = EnqueueMessage {
        task_ids: vec![0],
        enqueue_at: None,
    };
    send_message(shared, enqueue_message)
        .await
        .context("Failed to to add task message")?;

    // Make sure the task is started after being enqueued
    let task = wait_for_task_condition(shared, 0, |task| task.is_running()).await?;

    assert!(
        task.enqueued_at.unwrap() > pre_enqueue_time,
        "Enqueued tasks should have an enqeued_at time set."
    );

    Ok(())
}

/// Delayed stashed tasks will be enqueued.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_delayed_tasks() -> Result<()> {
    let daemon = daemon().await?;
    let shared = &daemon.settings.shared;

    // The task will be stashed and automatically enqueued after about 1 second.
    let response = add_stashed_task(
        shared,
        "sleep 10",
        true,
        Some(Local::now() + Duration::seconds(1)),
    )
    .await?;
    assert_success(response);

    // The task should be added in stashed state for about 1 second.
    wait_for_task_condition(shared, 0, |task| {
        matches!(task.status, TaskStatus::Stashed { .. })
    })
    .await?;

    // Make sure the task is started after being automatically enqueued.
    sleep_ms(800).await;
    wait_for_task_condition(shared, 0, |task| task.is_running()).await?;

    Ok(())
}

/// Stash a task that's currently queued for execution.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_stash_queued_task() -> Result<()> {
    let daemon = daemon().await?;
    let shared = &daemon.settings.shared;

    // Pause the daemon
    pause_tasks(shared, TaskSelection::All).await?;
    wait_for_group_status(shared, "default", GroupStatus::Paused).await?;

    // Add a task that's queued for execution.
    add_task(shared, "sleep 10", false).await?;

    // Stash the task
    send_message(shared, Message::Stash(vec![0]))
        .await
        .context("Failed to send STash message")?;

    let task = get_task(shared, 0).await?;
    assert_eq!(task.status, TaskStatus::Stashed { enqueue_at: None });
    assert!(
        task.enqueued_at.is_none(),
        "Enqueued tasks shouldn't have an enqeued_at date set."
    );

    Ok(())
}
