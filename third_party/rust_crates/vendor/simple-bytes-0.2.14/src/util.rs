
use std::io;
use std::error::Error;

pub(crate) fn io_other<E>(error: E) -> io::Error
where E: Into<Box<dyn Error + Send + Sync>> {
	io::Error::new(io::ErrorKind::Other, error)
}

pub(crate) fn io_eof<E>(error: E) -> io::Error
where E: Into<Box<dyn Error + Send + Sync>> {
	io::Error::new(io::ErrorKind::UnexpectedEof, error)
}

pub(crate) fn seek_from_to_n_pos(
	inner_len: usize,
	pos: usize,
	seek_from: io::SeekFrom
) -> io::Result<usize> {
	let n_pos = match seek_from {
		io::SeekFrom::Start(start) => start.try_into().map_err(io_eof)?,
		io::SeekFrom::End(end) => {
			let max: i64 = inner_len.try_into().map_err(io_other)?;
			let new = max - end;
			new.try_into().map_err(io_eof)?
		},
		io::SeekFrom::Current(curr) => {
			let pos: i64 = pos.try_into().map_err(io_other)?;
			let new = pos + curr;
			new.try_into().map_err(io_eof)?
		}
	};

	Ok(n_pos)
}

// returns the new position
pub(crate) fn write_or_alloc(
	vec: &mut Vec<u8>,
	pos: usize,
	slice: &[u8]
) -> usize {
	let rem_len = vec.len() - pos;

	// if has enough space
	if slice.len() <= rem_len {
		vec[pos..][..slice.len()].copy_from_slice(slice);
		return pos + slice.len()
	}

	// not enough space
	if rem_len > 0 {
		vec[pos..][..rem_len].copy_from_slice(&slice[..rem_len]);
	}

	vec.extend_from_slice(&slice[rem_len..]);
	pos + slice.len()
}