use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use tokio::{
    sync::{broadcast, RwLock},
    time::{sleep, Sleep},
};

#[derive(Debug)]
pub struct SwitchingSleep {
    period: Duration,
    tx: broadcast::Sender<()>,
    rx: broadcast::Receiver<()>,
    sleeper: Option<Sleep>,
}

impl SwitchingSleep {
    pub fn new(period: Duration) -> Self {
        let (tx, rx) = broadcast::channel(10);

        Self {
            period,
            tx,
            rx,
            sleeper: None,
        }
    }

    pub fn new_start(period: Duration) -> Self {
        let mut me = Self::new(period);
        me.start();
        me
    }

    pub fn start(&mut self) {
        if !self.is_elapsed() {
            self.stop();

            self.sleeper = Some(sleep(self.period));
            self.tx.send(()).unwrap();
        }
    }

    pub fn stop(&mut self) {
        if !self.is_elapsed() {
            match self.sleeper.take() {
                Some(_) => {
                    self.tx.send(()).unwrap();
                }
                None => (),
            }
        }
    }

    pub fn is_elapsed(&self) -> bool {
        self.sleeper.is_some() && (&self.sleeper).as_ref().unwrap().is_elapsed()
    }
}

unsafe impl Send for SwitchingSleep {}

impl Future for SwitchingSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<<Self as Future>::Output> {
        unsafe {
            let me = Pin::get_unchecked_mut(self);

            if me.is_elapsed() {
                return Poll::Ready(());
            }

            let sleeper = match me.sleeper {
                Some(ref mut sleeper) => {
                    let sleeper = Pin::new_unchecked(sleeper);

                    Some(sleeper.poll(cx))
                }
                None => None,
            };
            let mut recv = me.rx.recv();
            let recv = Pin::new_unchecked(&mut recv);
            let _ = recv.poll(cx);

            if let Some(Poll::Ready(_)) = sleeper {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }
}

#[derive(Debug)]
pub struct ASwitchingSleep(Arc<RwLock<SwitchingSleep>>);

impl ASwitchingSleep {
    pub fn new(period: Duration) -> Self {
        Self(Arc::new(RwLock::new(SwitchingSleep::new(period))))
    }

    pub async fn new_start(period: Duration) -> Self {
        let me = Self::new(period);
        me.start().await;
        me
    }

    pub async fn start(&self) {
        let mut inner = self.0.write().await;
        inner.start()
    }

    pub async fn stop(&self) {
        let mut inner = self.0.write().await;
        inner.stop()
    }

    pub async fn is_elapsed(&self) -> bool {
        let inner = self.0.read().await;
        inner.is_elapsed()
    }
}

unsafe impl Send for ASwitchingSleep {}
unsafe impl Sync for ASwitchingSleep {}

impl Clone for ASwitchingSleep {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Future for ASwitchingSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<<Self as Future>::Output> {
        unsafe {
            let me = Pin::get_unchecked_mut(self);

            let mut inner = me.0.write();
            let inner = Pin::new_unchecked(&mut inner);

            match inner.poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(mut inner) => Pin::new_unchecked(&mut *inner).poll(cx),
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::time::Duration;
    use tokio::{
        select,
        time::{sleep, Instant},
    };

    #[tokio::test]
    async fn it_works() {
        let mut sleeper = ASwitchingSleep::new(Duration::from_secs(3));

        let start = Instant::now();

        let mut task = {
            let sleeper = sleeper.clone();
            tokio::task::spawn(async move {
                sleep(Duration::from_secs(5)).await;

                assert_eq!(sleeper.is_elapsed().await, false);

                sleeper.start().await;

                sleep(Duration::from_secs(2)).await;

                assert_eq!(sleeper.is_elapsed().await, false);

                sleeper.stop().await;

                sleep(Duration::from_secs(2)).await;

                assert_eq!(sleeper.is_elapsed().await, false);

                sleeper.start().await;

                sleep(Duration::from_secs(2)).await;

                assert_eq!(sleeper.is_elapsed().await, false);
            })
        };

        loop {
            select! {
                _ = &mut task => {
                    loop {
                        select! {
                            _ = &mut sleeper => {
                                break;
                            }
                        }
                    }
                    break;
                },
                _ = &mut sleeper => break,
            }
        }

        let stop = Instant::now();
        let diff = stop - start;

        assert_eq!(sleeper.is_elapsed().await, true);
        assert_eq!(diff.as_secs(), 12);
    }
}
