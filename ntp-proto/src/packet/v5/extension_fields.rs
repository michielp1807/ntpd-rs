use crate::packet::v5::server_reference_id::BloomFilter;
use std::borrow::Cow;
use std::io::Write;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Type {
    DraftIdentification,
    Padding,
    Mac,
    ReferenceIdRequest,
    ReferenceIdResponse,
    ServerInformation,
    Correction,
    ReferenceTimestamp,
    MonotonicReceiveTimestamp,
    SecondaryReceiveTimestamp,
    Unknown(u16),
}

impl Type {
    pub const fn from_bits(bits: u16) -> Self {
        match bits {
            0xF5FF => Self::DraftIdentification,
            0xF501 => Self::Padding,
            0xF502 => Self::Mac,
            0xF503 => Self::ReferenceIdRequest,
            0xF504 => Self::ReferenceIdResponse,
            0xF505 => Self::ServerInformation,
            0xF506 => Self::Correction,
            0xF507 => Self::ReferenceTimestamp,
            0xF508 => Self::MonotonicReceiveTimestamp,
            0xF509 => Self::SecondaryReceiveTimestamp,
            other => Self::Unknown(other),
        }
    }

    pub const fn to_bits(self) -> u16 {
        match self {
            Self::DraftIdentification => 0xF5FF,
            Self::Padding => 0xF501,
            Self::Mac => 0xF502,
            Self::ReferenceIdRequest => 0xF503,
            Self::ReferenceIdResponse => 0xF504,
            Self::ServerInformation => 0xF505,
            Self::Correction => 0xF506,
            Self::ReferenceTimestamp => 0xF507,
            Self::MonotonicReceiveTimestamp => 0xF508,
            Self::SecondaryReceiveTimestamp => 0xF509,
            Self::Unknown(other) => other,
        }
    }

    #[cfg(test)]
    fn all_known() -> impl Iterator<Item = Self> {
        [
            Self::DraftIdentification,
            Self::Padding,
            Self::Mac,
            Self::ReferenceIdRequest,
            Self::ReferenceIdResponse,
            Self::ServerInformation,
            Self::Correction,
            Self::ReferenceTimestamp,
            Self::MonotonicReceiveTimestamp,
            Self::SecondaryReceiveTimestamp,
        ]
        .into_iter()
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct ReferenceIdRequest {
    payload_len: u16,
    offset: u16,
}

impl ReferenceIdRequest {
    pub fn new(payload_len: u16, offset: u16) -> Option<Self> {
        if payload_len % 4 != 0 {
            return None;
        }

        if payload_len + offset > 512 {
            return None;
        }

        Some(Self {
            payload_len,
            offset,
        })
    }

    pub fn to_response<'filter>(
        &self,
        filter: &'filter BloomFilter,
    ) -> Option<ReferenceIdResponse<'filter>> {
        let offset = usize::from(self.offset);
        let payload_len = usize::from(self.payload_len);

        let bytes = filter
            .as_bytes()
            .as_slice()
            .get(offset..)?
            .get(..payload_len)?
            .into();

        Some(ReferenceIdResponse { bytes })
    }

    pub fn serialize(&self, mut writer: impl Write) -> std::io::Result<()> {
        writer.write_all(&Type::ReferenceIdRequest.to_bits().to_be_bytes())?;
        writer.write_all(&self.offset.to_be_bytes())?;
        writer.write_all(&[0; 2])?;

        let words = self.payload_len / 4;
        assert_eq!(self.payload_len % 4, 0);
        for _ in 1..words {
            writer.write_all(&[0; 4])?;
        }

        Ok(())
    }

    pub(crate) fn offset(&self) -> u16 {
        self.offset
    }

    pub(crate) fn payload_len(&self) -> u16 {
        self.payload_len
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReferenceIdResponse<'a> {
    bytes: Cow<'a, [u8]>,
}

impl<'a> ReferenceIdResponse<'a> {
    pub fn new(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() % 4 != 0 {
            return None;
        }

        if bytes.len() > 512 {
            return None;
        }

        Some(Self {
            bytes: Cow::Borrowed(bytes),
        })
    }

    pub fn into_owned(self) -> ReferenceIdResponse<'static> {
        ReferenceIdResponse {
            bytes: Cow::Owned(self.bytes.into_owned()),
        }
    }

    pub fn serialize(&self, mut writer: impl Write) -> std::io::Result<()> {
        let len: u16 = self.bytes.len().try_into().unwrap();
        let len = len + 4; // Add room for type and length
        assert_eq!(len % 4, 0);

        writer.write_all(&Type::ReferenceIdResponse.to_bits().to_be_bytes())?;
        writer.write_all(&len.to_be_bytes())?;
        writer.write_all(self.bytes.as_ref())?;

        Ok(())
    }

    pub fn bytes(&self) -> &[u8] {
        &*self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_round_trip() {
        for i in 0..=u16::MAX {
            let ty = Type::from_bits(i);
            assert_eq!(i, ty.to_bits());
        }

        for ty in Type::all_known() {
            let bits = ty.to_bits();
            let ty2 = Type::from_bits(bits);
            assert_eq!(ty, ty2);

            let bits2 = ty2.to_bits();
            assert_eq!(bits, bits2);
        }
    }
}
