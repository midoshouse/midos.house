#![allow(unused_qualifications)]

use std::{
    ops::Deref,
    sync::Arc,
};
#[cfg(debug_assertions)] use std::ops::DerefMut;
#[cfg(not(debug_assertions))] pub(crate) use tokio::sync::{
    Mutex,
    OwnedRwLockWriteGuard,
};

#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct Mutex<T: ?Sized>(tokio::sync::Mutex<T>);

#[cfg(debug_assertions)] impl<T> Mutex<T> {
    pub(crate) fn new(t: T) -> Self {
        Self(tokio::sync::Mutex::new(t))
    }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Mutex<T> {
    pub(crate) async fn lock(&self) -> MutexGuard<'_, T> {
        println!("acquiring mutex guard");
        let guard = self.0.lock().await;
        println!("mutex guard acquired");
        MutexGuard(guard)
    }
}

#[cfg(debug_assertions)] pub(crate) struct MutexGuard<'a, T: ?Sized>(tokio::sync::MutexGuard<'a, T>);

#[cfg(debug_assertions)] impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T { &self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { &mut self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        println!("dropping mutex guard");
    }
}

pub(crate) struct ArcRwLock<T: ?Sized>(Arc<tokio::sync::RwLock<T>>);

impl<T> ArcRwLock<T> {
    pub(crate) fn new(t: T) -> Self {
        Self(Arc::new(tokio::sync::RwLock::new(t)))
    }
}

#[cfg(not(debug_assertions))] impl<T: ?Sized> ArcRwLock<T> {
    pub(crate) async fn write_owned(self) -> OwnedRwLockWriteGuard<T> {
        self.0.write_owned().await
    }
}

#[cfg(not(debug_assertions))] impl<T: ?Sized> Deref for ArcRwLock<T> {
    type Target = tokio::sync::RwLock<T>;

    fn deref(&self) -> &tokio::sync::RwLock<T> {
        &self.0
    }
}

#[cfg(debug_assertions)] impl<T: ?Sized> ArcRwLock<T> {
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, T> {
        println!("acquiring RwLock read guard");
        let guard = self.0.read().await;
        println!("RwLock read guard acquired");
        RwLockReadGuard(guard)
    }

    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, T> {
        println!("acquiring RwLock write guard");
        let guard = self.0.write().await;
        println!("RwLock write guard acquired");
        RwLockWriteGuard(guard)
    }

    pub(crate) async fn write_owned(self) -> OwnedRwLockWriteGuard<T> {
        println!("acquiring owned RwLock write guard");
        let guard = self.0.write_owned().await;
        println!("owned RwLock write guard acquired");
        OwnedRwLockWriteGuard(guard)
    }
}

impl<T> From<Arc<tokio::sync::RwLock<T>>> for ArcRwLock<T> {
    fn from(value: Arc<tokio::sync::RwLock<T>>) -> Self {
        Self(value)
    }
}

impl<T: ?Sized> Clone for ArcRwLock<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(debug_assertions)] pub(crate) struct RwLockReadGuard<'a, T: ?Sized>(tokio::sync::RwLockReadGuard<'a, T>);

#[cfg(debug_assertions)] impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T { &self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        println!("dropping RwLock read guard");
    }
}

#[cfg(debug_assertions)] pub(crate) struct RwLockWriteGuard<'a, T: ?Sized>(tokio::sync::RwLockWriteGuard<'a, T>);

#[cfg(debug_assertions)] impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T { &self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { &mut self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        println!("dropping RwLock write guard");
    }
}

#[cfg(debug_assertions)] pub(crate) struct OwnedRwLockWriteGuard<T: ?Sized>(tokio::sync::OwnedRwLockWriteGuard<T>);

#[cfg(debug_assertions)] impl<T: ?Sized> Deref for OwnedRwLockWriteGuard<T> {
    type Target = T;

    fn deref(&self) -> &T { &self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> DerefMut for OwnedRwLockWriteGuard<T> {
    fn deref_mut(&mut self) -> &mut T { &mut self.0 }
}

#[cfg(debug_assertions)] impl<T: ?Sized> Drop for OwnedRwLockWriteGuard<T> {
    fn drop(&mut self) {
        println!("dropping owned RwLock write guard");
    }
}
