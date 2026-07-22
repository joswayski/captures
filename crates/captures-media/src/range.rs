use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteRange {
    pub start: u64,
    pub end_inclusive: u64,
}

impl ByteRange {
    pub fn parse(value: &str, total_length: u64) -> Result<Self, ByteRangeError> {
        if total_length == 0 {
            return Err(ByteRangeError::Unsatisfiable);
        }
        let value = value
            .strip_prefix("bytes=")
            .ok_or(ByteRangeError::Invalid)?;
        if value.contains(',') {
            return Err(ByteRangeError::MultipleRangesUnsupported);
        }
        let (start, end) = value.split_once('-').ok_or(ByteRangeError::Invalid)?;
        if start.is_empty() {
            let suffix = end.parse::<u64>().map_err(|_| ByteRangeError::Invalid)?;
            if suffix == 0 {
                return Err(ByteRangeError::Unsatisfiable);
            }
            let suffix = suffix.min(total_length);
            return Ok(Self {
                start: total_length - suffix,
                end_inclusive: total_length - 1,
            });
        }

        let start = start.parse::<u64>().map_err(|_| ByteRangeError::Invalid)?;
        if start >= total_length {
            return Err(ByteRangeError::Unsatisfiable);
        }
        let end_inclusive = if end.is_empty() {
            total_length - 1
        } else {
            end.parse::<u64>()
                .map_err(|_| ByteRangeError::Invalid)?
                .min(total_length - 1)
        };
        if end_inclusive < start {
            return Err(ByteRangeError::Unsatisfiable);
        }
        Ok(Self {
            start,
            end_inclusive,
        })
    }

    pub const fn length(self) -> u64 {
        self.end_inclusive - self.start + 1
    }

    pub fn content_range(self, total_length: u64) -> String {
        format!(
            "bytes {}-{}/{}",
            self.start, self.end_inclusive, total_length
        )
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ByteRangeError {
    #[error("invalid byte range")]
    Invalid,
    #[error("multiple byte ranges are not supported")]
    MultipleRangesUnsupported,
    #[error("byte range is not satisfiable")]
    Unsatisfiable,
}

#[cfg(test)]
mod tests {
    use super::{ByteRange, ByteRangeError};

    #[test]
    fn parses_bounded_open_and_suffix_ranges() {
        assert_eq!(
            ByteRange::parse("bytes=10-19", 100),
            Ok(ByteRange {
                start: 10,
                end_inclusive: 19,
            })
        );
        assert_eq!(
            ByteRange::parse("bytes=90-", 100),
            Ok(ByteRange {
                start: 90,
                end_inclusive: 99,
            })
        );
        assert_eq!(
            ByteRange::parse("bytes=-10", 100),
            Ok(ByteRange {
                start: 90,
                end_inclusive: 99,
            })
        );
    }

    #[test]
    fn clamps_end_and_rejects_unsatisfiable_ranges() {
        let range = ByteRange::parse("bytes=95-200", 100).expect("range is clamped");
        assert_eq!(range.length(), 5);
        assert_eq!(range.content_range(100), "bytes 95-99/100");
        assert_eq!(
            ByteRange::parse("bytes=100-101", 100),
            Err(ByteRangeError::Unsatisfiable)
        );
    }
}
