//! Coroutine contract: the `MsgraphCoroutine` trait, its `MsgraphYield` /
//! `MsgraphCoroutineState`, and the `msgraph_try!` macro (the coroutine
//! equivalent of `?`).

use alloc::vec::Vec;

/// State returned by every [`MsgraphCoroutine::resume`] call: either an
/// I/O request to fulfil, or the coroutine's final value.
#[derive(Debug)]
pub enum MsgraphCoroutineState<Y, R> {
    /// The coroutine needs I/O before it can progress.
    Yielded(Y),
    /// The coroutine is done; resuming it again is a logic error.
    Complete(R),
}

/// An I/O-free Microsoft Graph coroutine, resumed with the bytes read
/// by the caller (or `None` when there is nothing to feed back).
pub trait MsgraphCoroutine {
    /// The I/O request type yielded while the coroutine progresses.
    type Yield;

    /// The final value produced on completion.
    type Return;

    /// Advances the coroutine with the outcome of the previous yield.
    fn resume(&mut self, arg: Option<&[u8]>) -> MsgraphCoroutineState<Self::Yield, Self::Return>;
}

/// I/O request yielded by a Microsoft Graph coroutine: a Graph call is
/// I/O-only, so reading and writing bytes are the only requests.
#[derive(Debug)]
pub enum MsgraphYield {
    /// The caller reads from its stream and resumes with the bytes.
    WantsRead,
    /// The caller writes the given bytes to its stream and resumes.
    WantsWrite(Vec<u8>),
}

/// Coroutine equivalent of `?`: forwards a `Yielded` state and
/// short-circuits a `Complete(Err(_))`, unwrapping a `Complete(Ok(_))`.
#[macro_export]
macro_rules! msgraph_try {
    ($coroutine:expr, $arg:expr $(,)?) => {
        match $crate::coroutine::MsgraphCoroutine::resume($coroutine, $arg) {
            $crate::coroutine::MsgraphCoroutineState::Yielded(y) => {
                return $crate::coroutine::MsgraphCoroutineState::Yielded(y.into());
            }
            $crate::coroutine::MsgraphCoroutineState::Complete(Err(err)) => {
                log::trace!("error during coroutine execution: {err}");
                return $crate::coroutine::MsgraphCoroutineState::Complete(Err(err.into()));
            }
            $crate::coroutine::MsgraphCoroutineState::Complete(Ok(value)) => value,
        }
    };
}
