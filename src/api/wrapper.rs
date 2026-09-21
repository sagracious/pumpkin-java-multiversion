use std::borrow::Cow;

use crate::api::TranslateError;
use crate::api::types::WireType;
use crate::packet::mappings::PacketId;

pub struct Translated {
    pub packet: &'static PacketId,
    pub payload: Vec<u8>,
    pub extra: Vec<(&'static PacketId, Vec<u8>)>,
    pub replies: Vec<(&'static PacketId, Vec<u8>)>,
}

pub struct PacketWrapper<'a> {
    packet: &'static PacketId,
    input: Cow<'a, [u8]>,
    pos: usize,
    output: Vec<u8>,
    cancelled: bool,
    extra: Vec<(&'static PacketId, Vec<u8>)>,
    replies: Vec<(&'static PacketId, Vec<u8>)>,
}

impl<'a> PacketWrapper<'a> {
    #[must_use]
    pub fn new(packet: &'static PacketId, payload: &'a [u8]) -> Self {
        Self {
            packet,
            input: Cow::Borrowed(payload),
            pos: 0,
            output: Vec::with_capacity(payload.len()),
            cancelled: false,
            extra: Vec::new(),
            replies: Vec::new(),
        }
    }

    #[must_use]
    pub const fn packet(&self) -> &'static PacketId {
        self.packet
    }

    pub const fn set_packet(&mut self, packet: &'static PacketId) {
        self.packet = packet;
    }

    pub fn read<T: WireType>(&mut self, t: &T) -> Result<T::Value, TranslateError> {
        let total = self.input.len();
        let mut cursor = &self.input[self.pos..];
        let value = t.read(&mut cursor)?;
        self.pos = total - cursor.len();
        Ok(value)
    }

    pub fn write<T: WireType>(&mut self, t: &T, v: &T::Value) -> Result<(), TranslateError> {
        t.write(&mut self.output, v)?;
        Ok(())
    }

    pub fn passthrough<T: WireType>(&mut self, t: &T) -> Result<T::Value, TranslateError> {
        let value = self.read(t)?;
        self.write(t, &value)?;
        Ok(value)
    }

    pub fn passthrough_all(&mut self) {
        self.output.extend_from_slice(&self.input[self.pos..]);
        self.pos = self.input.len();
    }

    #[must_use]
    pub fn remaining(&self) -> &[u8] {
        &self.input[self.pos..]
    }

    pub fn consume_remaining(&mut self) {
        self.pos = self.input.len();
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    /// Consumes the rest of the input and writes `payload` in its place,
    /// taking the buffer over when nothing has been written yet.
    pub fn replace_remaining(&mut self, payload: Vec<u8>) {
        self.pos = self.input.len();
        if self.output.is_empty() {
            self.output = payload;
        } else {
            self.output.extend_from_slice(&payload);
        }
    }

    pub const fn cancel(&mut self) {
        self.cancelled = true;
    }

    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn send_extra(&mut self, packet: &'static PacketId, payload: Vec<u8>) {
        self.extra.push((packet, payload));
    }

    /// Answers a serverbound packet with one the client gets straight back.
    pub fn send_reply(&mut self, packet: &'static PacketId, payload: Vec<u8>) {
        self.replies.push((packet, payload));
    }

    /// Makes what has been written so far the input of the next step. A step
    /// that has not read or written anything leaves the input as it is.
    pub fn reset(&mut self) {
        if self.pos == 0 && self.output.is_empty() {
            return;
        }
        self.input = Cow::Owned(std::mem::take(&mut self.output));
        self.pos = 0;
    }

    /// `Ok(None)` when cancelled, `Err(TrailingBytes)` when input is left.
    pub fn finish(self) -> Result<Option<Translated>, TranslateError> {
        if self.cancelled {
            return Ok(None);
        }
        let left = self.input.len() - self.pos;
        if left != 0 {
            return Err(TranslateError::TrailingBytes(left));
        }
        Ok(Some(Translated {
            packet: self.packet,
            payload: self.output,
            extra: self.extra,
            replies: self.replies,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{I32T, VAR_INT};
    use crate::packet::mappings::clientbound::play::{BLOCK_UPDATE, SET_SCORE};
    use pumpkin_protocol::codec::var_int::VarInt;

    #[test]
    fn a_passthrough_copies_and_finish_accepts_an_empty_input() {
        let mut wrapper = PacketWrapper::new(&BLOCK_UPDATE, &[0x01, 0x00, 0x00, 0x00, 0x07]);
        assert_eq!(wrapper.passthrough(&VAR_INT).unwrap().0, 1);
        assert_eq!(wrapper.passthrough(&I32T).unwrap(), 7);
        let out = wrapper.finish().unwrap().unwrap();
        assert_eq!(out.payload, [0x01, 0x00, 0x00, 0x00, 0x07]);
    }

    #[test]
    fn finish_rejects_trailing_input() {
        let mut wrapper = PacketWrapper::new(&BLOCK_UPDATE, &[0x01, 0x02]);
        wrapper.passthrough(&VAR_INT).unwrap();
        assert!(matches!(
            wrapper.finish(),
            Err(TranslateError::TrailingBytes(1))
        ));
    }

    #[test]
    fn reset_feeds_the_next_step_what_the_last_one_wrote() {
        let mut wrapper = PacketWrapper::new(&BLOCK_UPDATE, &[0x01]);
        wrapper.read(&VAR_INT).unwrap();
        wrapper.write(&VAR_INT, &VarInt(9)).unwrap();
        wrapper.reset();
        assert_eq!(wrapper.remaining(), [0x09]);
        wrapper.set_packet(&SET_SCORE);
        wrapper.passthrough_all();
        let out = wrapper.finish().unwrap().unwrap();
        assert_eq!(out.payload, [0x09]);
        assert_eq!(
            std::ptr::from_ref(out.packet),
            std::ptr::from_ref(&SET_SCORE)
        );
    }

    #[test]
    fn a_cancelled_wrapper_finishes_as_none() {
        let mut wrapper = PacketWrapper::new(&BLOCK_UPDATE, &[0x01]);
        wrapper.cancel();
        assert!(wrapper.finish().unwrap().is_none());
    }
}
