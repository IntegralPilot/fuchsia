
use std::fmt;

/// Get's returned when there is not enough data left to seek to the position.
///
/// The max position get's returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekError(pub usize);

impl fmt::Display for SeekError {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		fmt::Debug::fmt(self, f)
	}
}

impl std::error::Error for SeekError {}


/// Sets the internal position for writing or reading.
pub trait BytesSeek {
	/// Returns the internal position.
	fn position(&self) -> usize;

	/// Sets the internal position if possible.
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError>;

	/// Sets the internal position.
	#[track_caller]
	fn seek(&mut self, pos: usize) {
		self.try_seek(pos).expect("failed to seek");
	}

	/// Advances the internal position if possible.
	fn try_advance(&mut self, adv: usize) -> Result<(), SeekError> {
		self.try_seek(self.position() + adv)
	}

	/// Advances the internal position.
	/// 
	/// ## Panic
	/// May panic depending on the `BytesSeek::seek` implementation.
	#[track_caller]
	fn advance(&mut self, adv: usize) {
		self.try_advance(adv).expect("failed to advance")
	}
}

impl<S: BytesSeek> BytesSeek for &mut S {
	#[inline]
	fn position(&self) -> usize {
		(**self).position()
	}

	#[inline]
	fn try_seek(&mut self, pos: usize) -> Result<(), SeekError> {
		(**self).try_seek(pos)
	}
}