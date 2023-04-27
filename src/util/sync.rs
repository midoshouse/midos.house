#![allow(unused_qualifications)]

use {
    std::{
        ops::{
            Deref,
            DerefMut,
        },
        sync::Arc,
    },
    derivative::Derivative,
};

macro_rules! lock {
    ($mutex:expr) => {{
        #[allow(unused_qualifications)] {
            #[cfg(debug_assertions)] println!(
                "[{} {}:{}] acquiring mutex guard",
                file!(),
                line!(),
                column!(),
            );
            let mut guard_fut = std::pin::pin!($mutex.0.lock());
            let guard = tokio::select! {
                guard = &mut guard_fut => guard,
                () = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                    eprintln!(
                        "[{} {}:{}] warning: acquiring mutex guard taking over a minute",
                        file!(),
                        line!(),
                        column!(),
                    );
                    guard_fut.await
                }
            };
            #[cfg(debug_assertions)] println!(
                "[{} {}:{}] mutex guard acquired",
                file!(),
                line!(),
                column!(),
            );
            $crate::util::sync::MutexGuard(guard)
        }
    }};
    (@read $rw_lock:expr) => {{
        #[allow(unused_qualifications)] {
            #[cfg(debug_assertions)] println!(
                "[{} {}:{}] acquiring RwLock read guard",
                file!(),
                line!(),
                column!(),
            );
            let mut guard_fut = std::pin::pin!($rw_lock.0.read());
            let guard = tokio::select! {
                guard = &mut guard_fut => guard,
                () = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                    eprintln!(
                        "[{} {}:{}] warning: acquiring RwLock read guard taking over a minute",
                        file!(),
                        line!(),
                        column!(),
                    );
                    guard_fut.await
                }
            };
            #[cfg(debug_assertions)] println!(
                "[{} {}:{}] RwLock read guard acquired",
                file!(),
                line!(),
                column!(),
            );
            $crate::util::sync::RwLockReadGuard(guard)
        }
    }};
    (@write $rw_lock:expr) => {{
        #[allow(unused_qualifications)] {
            #[cfg(debug_assertions)] println!(
                "[{} {}:{}] acquiring RwLock write guard",
                file!(),
                line!(),
                column!(),
            );
            let mut guard_fut = std::pin::pin!($rw_lock.0.write());
            let guard = tokio::select! {
                guard = &mut guard_fut => guard,
                () = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                    eprintln!(
                        "[{} {}:{}] warning: acquiring RwLock write guard taking over a minute",
                        file!(),
                        line!(),
                        column!(),
                    );
                    guard_fut.await
                }
            };
            #[cfg(debug_assertions)] println!(
                "[{} {}:{}] RwLock write guard acquired",
                file!(),
                line!(),
                column!(),
            );
            $crate::util::sync::RwLockWriteGuard(guard)
        }
    }};
    (@write_owned $rw_lock:expr) => {{
        #[allow(unused_qualifications)] {
            #[cfg(debug_assertions)] println!(
                "[{} {}:{}] acquiring owned RwLock write guard",
                file!(),
                line!(),
                column!(),
            );
            let mut guard_fut = std::pin::pin!($rw_lock.0.write_owned());
            let guard = tokio::select! {
                guard = &mut guard_fut => guard,
                () = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                    eprintln!(
                        "[{} {}:{}] warning: acquiring owned RwLock write guard taking over a minute",
                        file!(),
                        line!(),
                        column!(),
                    );
                    guard_fut.await
                }
            };
            #[cfg(debug_assertions)] println!(
                "[{} {}:{}] owned RwLock write guard acquired",
                file!(),
                line!(),
                column!(),
            );
            $crate::util::sync::OwnedRwLockWriteGuard(guard)
        }
    }};
}

pub(crate) use lock;

#[derive(Debug, Default)]
pub(crate) struct Mutex<T: ?Sized>(pub(crate) tokio::sync::Mutex<T>);

impl<T> Mutex<T> {
    pub(crate) fn new(t: T) -> Self {
        Self(tokio::sync::Mutex::new(t))
    }

    pub(crate) fn into_inner(self) -> T {
        self.0.into_inner()
    }
}

pub(crate) struct MutexGuard<'a, T: ?Sized>(pub(crate) tokio::sync::MutexGuard<'a, T>);

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T { &self.0 }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { &mut self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        println!("dropping mutex guard");
    }
}

pub(crate) struct RwLock<T: ?Sized>(pub(crate) tokio::sync::RwLock<T>);

impl<T> RwLock<T> {
    pub(crate) fn new(t: T) -> Self {
        Self(tokio::sync::RwLock::new(t))
    }
}

pub(crate) struct RwLockReadGuard<'a, T: ?Sized>(pub(crate) tokio::sync::RwLockReadGuard<'a, T>);

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T { &self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        println!("dropping RwLock read guard");
    }
}

pub(crate) struct RwLockWriteGuard<'a, T: ?Sized>(pub(crate) tokio::sync::RwLockWriteGuard<'a, T>);

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T { &self.0 }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { &mut self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        println!("dropping RwLock write guard");
    }
}

#[derive(Derivative)]
#[derivative(Clone(bound = ""))]
pub(crate) struct ArcRwLock<T: ?Sized>(pub(crate) Arc<tokio::sync::RwLock<T>>);

impl<T> ArcRwLock<T> {
    pub(crate) fn new(t: T) -> Self {
        Self(Arc::new(tokio::sync::RwLock::new(t)))
    }
}

impl<T> From<Arc<tokio::sync::RwLock<T>>> for ArcRwLock<T> {
    fn from(value: Arc<tokio::sync::RwLock<T>>) -> Self {
        Self(value)
    }
}

pub(crate) struct OwnedRwLockWriteGuard<T: ?Sized>(pub(crate) tokio::sync::OwnedRwLockWriteGuard<T>);

impl<T: ?Sized> Deref for OwnedRwLockWriteGuard<T> {
    type Target = T;

    fn deref(&self) -> &T { &self.0 }
}

impl<T: ?Sized> DerefMut for OwnedRwLockWriteGuard<T> {
    fn deref_mut(&mut self) -> &mut T { &mut self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Drop for OwnedRwLockWriteGuard<T> {
    fn drop(&mut self) {
        println!("dropping owned RwLock write guard");
    }
}
