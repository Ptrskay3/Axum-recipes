use std::convert::Infallible;

use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::{state::AppState, utils::shutdown_signal};

#[tracing::instrument(skip_all)]
pub async fn sse_handler(
    State(AppState { tx: chan, .. }): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Create an internal channel which transmits all traffic that's coming from our `chan`.
    let (mut tx, rx) = futures::channel::mpsc::channel::<Result<Event, Infallible>>(16);

    let mut sub = chan.subscribe();
    tokio::spawn(async move {
        use futures::SinkExt;

        while let Ok(m) = sub.recv().await {
            if let Err(send_error) = tx
                .send(Ok(Event::default().event(m.name()).json_data(m).unwrap()))
                .await
            {
                tracing::trace!("Broadcasting error: {:?}", send_error);
            }
        }
    });

    Sse::new(or_until_shutdown(rx)).keep_alive(KeepAlive::default())
}

/// Run a stream until it completes or we receive the shutdown signal.
///
/// Uses the `async-stream` to make things easier to write.
fn or_until_shutdown<S>(stream: S) -> impl Stream<Item = S::Item>
where
    S: Stream,
{
    async_stream::stream! {
        futures::pin_mut!(stream);

        let shutdown_signal = shutdown_signal();
        futures::pin_mut!(shutdown_signal);

        loop {
            tokio::select! {
                Some(item) = stream.next() => {
                    yield item
                }
                _ = &mut shutdown_signal => {
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Notification {
    NewRecipe(NewRecipe),
}

impl Notification {
    pub fn new_recipe(name: String) -> Self {
        Self::NewRecipe(NewRecipe { name })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::NewRecipe(_) => "new_recipe",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRecipe {
    pub name: String,
}
