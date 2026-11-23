use crate::{
    ChoiceList, Correlation, Error, FailureCode, Message, Outcome, frame::FrameError,
    writer::Writer,
};
use std::{
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

mod private {
    use crate::Message;
    pub trait Sealed: Sized {
        fn into_message(self) -> Message;
        fn fallback() -> Self;
    }
}

/// What a [`Reply`] can carry: an [`Outcome`] for an action, a [`ChoiceList`] for an options query.
pub trait Answer: private::Sealed {}
impl Answer for Outcome {}
impl Answer for ChoiceList {}

impl private::Sealed for Outcome {
    fn into_message(self) -> Message {
        Message::Result { outcome: self }
    }
    fn fallback() -> Self {
        Outcome::Failure {
            code: FailureCode::Failed,
        }
    }
}
impl private::Sealed for ChoiceList {
    fn into_message(self) -> Message {
        let list = if self.is_valid() {
            self
        } else {
            ChoiceList::Unavailable
        };
        Message::Choices { list }
    }
    fn fallback() -> Self {
        ChoiceList::Unavailable
    }
}

pub(crate) struct Slots {
    in_use: AtomicUsize,
    limit: usize,
}
impl Slots {
    pub(crate) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            in_use: AtomicUsize::new(0),
            limit,
        })
    }

    pub(crate) fn acquire(self: &Arc<Self>) -> Option<Permit> {
        let mut current = self.in_use.load(Ordering::Acquire);
        loop {
            if current >= self.limit {
                return None;
            }
            match self.in_use.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Permit(Arc::clone(self))),
                Err(actual) => current = actual,
            }
        }
    }
}
pub(crate) struct Permit(Arc<Slots>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.in_use.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The pending answer to one request. It may be completed later and from any thread, and holds
/// one concurrency slot until it is sent or dropped. Dropping it unanswered answers with a
/// failure (`failed` for an action, `unavailable` for options), so the host never waits for a
/// request the plugin forgot.
pub struct Reply<T: Answer> {
    writer: Writer,
    correlation: Correlation,
    answered: bool,
    _credentials: Option<crate::Credentials>,
    _permit: Permit,
    _answer: PhantomData<fn(T)>,
}
impl<T: Answer> Reply<T> {
    pub(crate) fn new(
        writer: Writer,
        correlation: Correlation,
        permit: Permit,
        credentials: Option<crate::Credentials>,
    ) -> Self {
        Self {
            writer,
            correlation,
            answered: false,
            _credentials: credentials,
            _permit: permit,
            _answer: PhantomData,
        }
    }

    pub fn send(mut self, answer: T) {
        self._credentials.take();
        self.answered = true;
        self.write(answer);
    }

    fn write(&self, answer: T) {
        if let Err(Error::Frame(FrameError::TooLarge(_))) =
            self.writer.send(self.correlation, answer.into_message())
        {
            let _ = self
                .writer
                .send(self.correlation, T::fallback().into_message());
        }
    }
}
impl<T: Answer> Drop for Reply<T> {
    fn drop(&mut self) {
        self._credentials.take();
        if !self.answered {
            self.write(T::fallback());
        }
    }
}
