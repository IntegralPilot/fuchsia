// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::{Deref, DerefMut};

/// An `Augmented` value is a generic wrapper that holds a primary value of type `T` and an
/// optional auxiliary value of type `A`.
///
/// This is useful when a function needs to return a value that has some additional, optional
/// context attached to it. For example, `A` can be used to override some of the fields of `T`
/// without modifying `T` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Augmented<T, A: Clone> {
    /// The primary value, without any auxiliary data.
    Primary(T),
    /// The primary value, with auxiliary data.
    WithAux(T, A),
}

impl<T, A: Clone> Augmented<T, A> {
    /// Maps an `Augmented<T, A>` to an `Augmented<U, A>` by applying a function to the contained
    /// primary value, leaving the auxiliary value untouched.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Augmented<U, A> {
        match self {
            Self::Primary(t) => Augmented::Primary(f(t)),
            Self::WithAux(t, aux) => Augmented::WithAux(f(t), aux),
        }
    }

    /// Extracts the primary value from the `Augmented` value, discarding the auxiliary value if it
    /// exists.
    pub fn extract(self) -> T {
        match self {
            Self::Primary(t) => t,
            Self::WithAux(t, _) => t,
        }
    }

    /// Converts an `Augmented<T, A>` to an `Augmented<&T, A>`.
    pub fn as_ref(&self) -> Augmented<&T, A> {
        match self {
            Self::Primary(t) => Augmented::Primary(t),
            Self::WithAux(t, aux) => Augmented::WithAux(t, aux.clone()),
        }
    }

    /// Converts an `Augmented<T, A>` to an `Augmented<&mut T, A>`.
    pub fn as_mut(&mut self) -> Augmented<&mut T, A> {
        match self {
            Self::Primary(t) => Augmented::Primary(t),
            Self::WithAux(t, aux) => Augmented::WithAux(t, aux.clone()),
        }
    }
}

impl<T, A: Clone> Augmented<&mut T, A> {
    /// Converts an `Augmented<&mut T, A>` to an `Augmented<&T, A>`.
    pub fn as_unmut(&self) -> Augmented<&T, A> {
        match self {
            Self::Primary(t) => Augmented::Primary(t),
            Self::WithAux(t, aux) => Augmented::WithAux(t, aux.clone()),
        }
    }
}

impl<T, A: Clone> Augmented<Option<T>, A> {
    /// Transposes an `Augmented<Option<T>, A>` into an `Option<Augmented<T, A>>`.
    pub fn transpose(self) -> Option<Augmented<T, A>> {
        match self {
            Self::Primary(t) => Some(Augmented::Primary(t?)),
            Self::WithAux(t, aux) => Some(Augmented::WithAux(t?, aux)),
        }
    }
}

impl<T, A: Clone, E> Augmented<Result<T, E>, A> {
    /// Transposes an `Augmented<Result<T, E>, A>` into a `Result<Augmented<T, A>, E>`.
    pub fn transpose(self) -> Result<Augmented<T, A>, E> {
        match self {
            Self::Primary(t) => Ok(Augmented::Primary(t?)),
            Self::WithAux(t, aux) => Ok(Augmented::WithAux(t?, aux)),
        }
    }
}

impl<T, A: Clone> From<T> for Augmented<T, A> {
    /// Creates a `Primary` `Augmented` value from a primary value.
    fn from(t: T) -> Self {
        Self::Primary(t)
    }
}

impl<T, A: Clone> Deref for Augmented<T, A> {
    type Target = T;

    /// Dereferences the `Augmented` value to the primary value.
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Primary(t) => &t,
            Self::WithAux(t, _) => &t,
        }
    }
}

impl<T, A: Clone> DerefMut for Augmented<T, A> {
    /// Mutably dereferences the `Augmented` value to the primary value.
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Primary(t) => t,
            Self::WithAux(t, _) => t,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map() {
        let primary = Augmented::<_, ()>::Primary(5);
        let mapped_primary = primary.map(|x| x * 2);
        assert!(matches!(mapped_primary, Augmented::<_, ()>::Primary(10)));

        let with_aux = Augmented::WithAux(5, "hello");
        let mapped_with_aux = with_aux.map(|x| x * 2);
        assert!(matches!(mapped_with_aux, Augmented::WithAux(10, "hello")));
    }

    #[test]
    fn test_extract() {
        assert_eq!(Augmented::<_, ()>::Primary(5).extract(), 5);
        assert_eq!(Augmented::WithAux(5, "hello").extract(), 5);
    }

    #[test]
    fn test_as_ref() {
        let primary = Augmented::<_, ()>::Primary(5);
        let ref_primary = primary.as_ref();
        assert!(matches!(ref_primary, Augmented::<_, ()>::Primary(&5)));

        let with_aux = Augmented::WithAux(5, "hello");
        let ref_with_aux = with_aux.as_ref();
        assert!(matches!(ref_with_aux, Augmented::WithAux(&5, "hello")));
    }

    #[test]
    fn test_as_mut() {
        let mut primary = Augmented::<_, ()>::Primary(5);
        if let Augmented::<_, ()>::Primary(val) = primary.as_mut() {
            *val = 10;
        }
        assert_eq!(*primary, 10);

        let mut with_aux = Augmented::WithAux(5, "hello");
        if let Augmented::WithAux(val, aux) = with_aux.as_mut() {
            assert_eq!(aux, "hello");
            *val = 10;
        }
        assert_eq!(*with_aux, 10);
    }

    #[test]
    fn test_as_unmut() {
        let mut primary = Augmented::<_, ()>::Primary(5);
        let mut_ref = primary.as_mut();
        let unmut_ref = mut_ref.as_unmut();
        assert!(matches!(unmut_ref, Augmented::<_, ()>::Primary(&5)));

        let mut with_aux = Augmented::WithAux(5, "hello");
        let mut_ref_aux = with_aux.as_mut();
        let unmut_ref_aux = mut_ref_aux.as_unmut();
        assert!(matches!(unmut_ref_aux, Augmented::WithAux(&5, "hello")));
    }

    #[test]
    fn test_transpose_option() {
        let primary_some = Augmented::<_, ()>::Primary(Some(5));
        let transposed_primary_some = primary_some.transpose();
        assert!(matches!(transposed_primary_some, Some(Augmented::<_, ()>::Primary(5))));

        let primary_none: Augmented<Option<i32>, &str> = Augmented::Primary(None);
        assert!(primary_none.transpose().is_none());

        let with_aux_some = Augmented::WithAux(Some(5), "hello");
        let transposed_with_aux_some = with_aux_some.transpose();
        assert!(matches!(transposed_with_aux_some, Some(Augmented::WithAux(5, "hello"))));

        let with_aux_none: Augmented<Option<i32>, &str> = Augmented::WithAux(None, "hello");
        assert!(with_aux_none.transpose().is_none());
    }

    #[test]
    fn test_transpose_result() {
        let primary_ok: Augmented<Result<i32, &str>, &str> = Augmented::Primary(Ok(5));
        let transposed_primary_ok = primary_ok.transpose();
        assert!(matches!(transposed_primary_ok, Ok(Augmented::Primary(5))));

        let primary_err: Augmented<Result<i32, &str>, &str> = Augmented::Primary(Err("error"));
        assert_eq!(primary_err.transpose(), Err("error"));

        let with_aux_ok: Augmented<Result<i32, &str>, &str> = Augmented::WithAux(Ok(5), "hello");
        let transposed_with_aux_ok = with_aux_ok.transpose();
        assert!(matches!(transposed_with_aux_ok, Ok(Augmented::WithAux(5, "hello"))));

        let with_aux_err: Augmented<Result<i32, &str>, &str> =
            Augmented::WithAux(Err("error"), "hello");
        assert_eq!(with_aux_err.transpose(), Err("error"));
    }

    #[test]
    fn test_from() {
        let augmented: Augmented<i32, &str> = 5.into();
        assert!(matches!(augmented, Augmented::Primary(5)));
    }

    #[test]
    fn test_deref() {
        assert_eq!(*Augmented::<_, ()>::Primary(5), 5);
        assert_eq!(*Augmented::WithAux(5, "hello"), 5);
    }

    #[test]
    fn test_deref_mut() {
        let mut primary = Augmented::<_, ()>::Primary(5);
        *primary = 10;
        assert_eq!(*primary, 10);

        let mut with_aux = Augmented::WithAux(5, "hello");
        *with_aux = 10;
        assert_eq!(*with_aux, 10);
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Data {
        a: i32,
        b: i32,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct DataOverride {
        b: Option<i32>,
    }

    fn get_b(augmented: &Augmented<Data, DataOverride>) -> i32 {
        match augmented {
            Augmented::Primary(data) => data.b,
            Augmented::WithAux(data, aux) => aux.b.unwrap_or(data.b),
        }
    }

    #[test]
    fn test_override_example() {
        let primary = Augmented::Primary(Data { a: 1, b: 2 });
        assert_eq!(get_b(&primary), 2);

        let with_aux_override =
            Augmented::WithAux(Data { a: 1, b: 2 }, DataOverride { b: Some(3) });
        assert_eq!(get_b(&with_aux_override), 3);

        let with_aux_no_override =
            Augmented::WithAux(Data { a: 1, b: 2 }, DataOverride { b: None });
        assert_eq!(get_b(&with_aux_no_override), 2);
    }
}
