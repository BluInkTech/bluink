// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Bounded pkt-line framing shared by pure SSH LFS commands and protocol tests.
use super::super::{Result, failure};
use std::io::{Read, Write};

pub(super) enum Packet {
    Flush,
    Delimiter,
    Data(Vec<u8>),
}

pub(super) fn read(stream: &mut impl Read) -> Result<Packet> {
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let length = std::str::from_utf8(&header)
        .ok()
        .and_then(|s| usize::from_str_radix(s, 16).ok())
        .ok_or_else(|| failure("Invalid SSH attachment framing"))?;
    match length {
        0 => Ok(Packet::Flush),
        1 => Ok(Packet::Delimiter),
        4..=65_520 => {
            let mut bytes = vec![0; length - 4];
            stream.read_exact(&mut bytes)?;
            Ok(Packet::Data(bytes))
        }
        _ => Err(failure("Invalid SSH attachment packet length")),
    }
}

pub(super) fn write(stream: &mut impl Write, bytes: &[u8]) -> Result<()> {
    if bytes.len() > 65_516 {
        return Err(failure("SSH attachment packet is too large"));
    }
    write!(stream, "{:04x}", bytes.len() + 4)?;
    stream.write_all(bytes)?;
    Ok(())
}

pub(super) fn command(
    stream: &mut impl Write,
    command: &str,
    args: &[String],
    data: bool,
) -> Result<()> {
    write(stream, format!("{command}\n").as_bytes())?;
    for arg in args {
        write(stream, format!("{arg}\n").as_bytes())?;
    }
    stream.write_all(if data { b"0001" } else { b"0000" })?;
    stream.flush()?;
    Ok(())
}

pub(super) fn section(stream: &mut impl Read) -> Result<(Vec<String>, bool)> {
    let mut lines = Vec::new();
    let mut size = 0;
    loop {
        match read(stream)? {
            Packet::Flush => return Ok((lines, false)),
            Packet::Delimiter => return Ok((lines, true)),
            Packet::Data(bytes) => {
                size += bytes.len();
                if size > 1024 * 1024 {
                    return Err(failure("SSH attachment response is too large"));
                }
                lines.push(
                    String::from_utf8(bytes)
                        .map_err(|_| failure("Invalid SSH attachment response"))?
                        .trim_end_matches('\n')
                        .to_owned(),
                );
            }
        }
    }
}

pub(super) fn status(stream: &mut impl Read) -> Result<(Vec<String>, bool)> {
    let (lines, delimiter) = section(stream)?;
    if !lines
        .first()
        .is_some_and(|line| line == "status 200" || line == "status 201")
    {
        return Err(failure("SSH attachment service rejected the operation"));
    }
    Ok((lines, delimiter))
}

pub(super) fn finish(stream: &mut impl Read) -> Result<()> {
    let (_, data) = status(stream)?;
    if data {
        let (body, delimiter) = section(stream)?;
        if delimiter || !body.is_empty() {
            return Err(failure("Unexpected SSH attachment response body"));
        }
    }
    Ok(())
}

pub(super) struct Body<'a, T: Read> {
    stream: &'a mut T,
    bytes: std::io::Cursor<Vec<u8>>,
    ended: bool,
}
impl<'a, T: Read> Body<'a, T> {
    pub(super) const fn new(stream: &'a mut T) -> Self {
        Self {
            stream,
            bytes: std::io::Cursor::new(Vec::new()),
            ended: false,
        }
    }
}
impl<T: Read> Read for Body<'_, T> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.bytes.read(buffer)?;
            if count > 0 || self.ended {
                return Ok(count);
            }
            match read(self.stream).map_err(std::io::Error::other)? {
                Packet::Flush => self.ended = true,
                Packet::Data(bytes) => self.bytes = std::io::Cursor::new(bytes),
                Packet::Delimiter => {
                    return Err(std::io::Error::other("Unexpected attachment delimiter"));
                }
            }
        }
    }
}
