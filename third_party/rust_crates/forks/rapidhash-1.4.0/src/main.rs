use std::io::{Read};

/// Command-line tool for rapidhash.
///
/// Rapidhash produces a `u64` hash value, and terminal output is a decimal string of the hash
/// value.
///
///
/// # Install
/// ```shell
/// cargo install rapidhash
/// ```
///
/// # Usage
///
/// ## Reading file
///
/// This will first check the metadata of the file to get the length, and then stream the file.
///
/// ```bash
/// rapidhash example.txt
/// 8543579700415218186
/// ```
///
/// ## Reading stdin
///
/// **NOTE:**
/// Because of how rapidhash is seeded using the data length, the length must be known at the start
/// of the stream. Therefore reading from stdin is not recommended, as it will cache the entire
/// input in memory before being able to hash it.
///
/// ```shell
/// echo "example" | rapidhash
/// 8543579700415218186
/// ```
pub fn main() {
    let hash_arg = std::env::args().nth(1);

    let hash = match hash_arg {
        None => {
            let mut buffer = Vec::with_capacity(1024);
            std::io::stdin().read_to_end(&mut buffer).expect("Could not read from stdin.");
            rapidhash::rapidhash(&buffer)
        }
        Some(filename) => {
            if filename == "--help" {
                println!("Usage: rapidhash [filename]");
                println!("Docs: https://github.com/hoxxep/rapidhash?tab=readme-ov-file#cli");
                return;
            }

            #[cfg(feature = "std")] {
                let mut file = std::fs::File::open(filename).expect("Could not open file.");
                rapidhash::rapidhash_file(&mut file).expect("Failed to hash file.")
            }

            #[cfg(not(feature = "std"))] {
                panic!("File reading is not supported without the `std` feature.");
            }
        }
    };

    println!("{hash}");
}
