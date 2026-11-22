//! `runtime/compression`: потоковые DEFLATE (RFC 1951), zlib (RFC 1950) и
//! gzip (RFC 1952) на miniz_oxide. Заголовок и хвост gzip (CRC-32, ISIZE)
//! строятся и проверяются здесь; Adler-32 zlib проверяет miniz_oxide. Один
//! объект сжимает или распаковывает один поток; Sparrow берёт эти natives
//! через мост синхронных natives (`native_origin`). zstd и Brotli (#184)
//! живут в подмодулях за теми же natives.
mod brotli_codec;
mod zstd_codec;

use std::cell::RefCell;
use std::rc::Rc;

use super::Error;
use crate::{Value, abi::Native, abi::NativeFailure, heap::ArrayHeap};
use miniz_oxide::deflate::core::{
    CompressorOxide, TDEFLFlush, TDEFLStatus, compress, create_comp_flags_from_zip_params,
};
use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

const CHUNK: usize = 64 * 1024;
/// The first output buffer for an input of `size` bytes; it grows to CHUNK
/// when filled, so a small value does not zero a whole CHUNK (#943).
fn first_buffer(size: usize) -> Vec<u8> {
    vec![0u8; size.saturating_add(1024).min(CHUNK)]
}

/// The next buffer after `buffer` was filled.
fn grown(buffer: &mut Vec<u8>) {
    if buffer.len() < CHUNK {
        buffer.resize((buffer.len() * 2).min(CHUNK), 0);
    }
}

/// Engines kept for the next codec on this thread. miniz builds hundreds of
/// KiB of tables per engine, and fresh ones are page-faulted in on every
/// one-shot value (#943): a finished codec hands its engine back, and a pooled
/// engine is reset to the state a new one starts in.
const POOLED: usize = 4;

thread_local! {
    static DEFLATERS: RefCell<Vec<Box<CompressorOxide>>> = const { RefCell::new(Vec::new()) };
    static INFLATERS: RefCell<Vec<Box<InflateState>>> = const { RefCell::new(Vec::new()) };
}

fn deflater(format: DataFormat, level: u8) -> Box<CompressorOxide> {
    match DEFLATERS.with(|pool| pool.borrow_mut().pop()) {
        Some(mut engine) => {
            engine.reset();
            engine.set_format_and_level(format, level);
            engine
        }
        None => Box::new(CompressorOxide::new(create_comp_flags_from_zip_params(
            level as i32,
            format.to_window_bits(),
            0,
        ))),
    }
}

fn inflater(format: DataFormat) -> Box<InflateState> {
    match INFLATERS.with(|pool| pool.borrow_mut().pop()) {
        Some(mut engine) => {
            engine.reset(format);
            engine
        }
        None => InflateState::new_boxed(format),
    }
}

/// Keeps `engine` for the next codec, unless the pool is full or already gone
/// (a codec dropped while the thread's locals are destroyed).
fn give_back<T>(pool: &'static std::thread::LocalKey<RefCell<Vec<Box<T>>>>, engine: Box<T>) {
    let _ = pool.try_with(|pool| {
        if let Ok(mut pool) = pool.try_borrow_mut()
            && pool.len() < POOLED
        {
            pool.push(engine);
        }
    });
}

/// Самый длинный заголовок gzip, который ждём целиком (FEXTRA, FNAME, FCOMMENT).
const HEADER_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Deflate,
    Zlib,
    Gzip,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Header,
    Body,
    Trailer,
    Ended,
}

/// Отказ кодека: код `core.compression` и сообщение.
struct Fail(&'static str, String);

fn invalid(message: impl Into<String>) -> Fail {
    Fail("invalid_input", message.into())
}

fn exceeded(limit: u64) -> Fail {
    Fail("resource_exhausted", format!("Decompressed data exceeds {limit} bytes"))
}

/// A codec of a format outside miniz_oxide (zstd, Brotli) behind the same
/// natives.
trait Stream {
    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, Fail>;
    fn finish(&mut self) -> Result<Vec<u8>, Fail>;
    /// A compressor has finished; a decompressor has met the end of its stream.
    fn ended(&self) -> bool;
    fn remainder(&self) -> Vec<u8>;
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 (ISO-HDLC) `crc`, продолженный байтами: crc32(crc32(0, a), b) ==
/// crc32(0, a ++ b).
fn crc32(crc: u32, bytes: &[u8]) -> u32 {
    let mut c = !crc;
    for &byte in bytes {
        c = CRC_TABLE[((c ^ byte as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// CRC-16/CCITT register `crc` (polynomial 0x1021, MSB first, no reflection,
/// no final xor) continued by bytes (#193).
fn crc16(crc: u16, bytes: &[u8]) -> u16 {
    let mut c = crc;
    for &byte in bytes {
        c ^= u16::from(byte) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x1021 } else { c << 1 };
        }
    }
    c
}

/// Adler-32 (RFC 1950) packed sums `seed` continued by bytes; both halves of
/// the seed are first reduced modulo 65521, as zlib does (#193).
fn adler32(seed: u32, bytes: &[u8]) -> u32 {
    const MODULUS: u32 = 65521;
    // The largest run whose sums cannot overflow a u32 before reduction.
    const RUN: usize = 5552;
    let mut low = (seed & 0xFFFF) % MODULUS;
    let mut high = (seed >> 16) % MODULUS;
    for run in bytes.chunks(RUN) {
        for &byte in run {
            low += u32::from(byte);
            high += low;
        }
        low %= MODULUS;
        high %= MODULUS;
    }
    (high << 16) | low
}

/// The `initial` of a checksum native: 0..=`maximum`.
fn seed(native: Native, value: &Value, maximum: u32) -> Result<u32, Error> {
    super::support::int(value)?
        .to_i64()
        .and_then(|seed| u32::try_from(seed).ok())
        .filter(|seed| *seed <= maximum)
        .ok_or_else(|| failure(native, Fail("invalid_input", "checksum seed out of range".into())))
}

struct Compressor {
    /// Held until finish, then given back to the pool.
    engine: Option<Box<CompressorOxide>>,
    format: Format,
    crc: u32,
    size: u32,
    started: bool,
    finished: bool,
}

impl Compressor {
    fn new(format: Format, level: u8) -> Self {
        let data = if format == Format::Zlib { DataFormat::Zlib } else { DataFormat::Raw };
        Self {
            engine: Some(deflater(data, level)),
            format,
            crc: 0,
            size: 0,
            started: false,
            finished: false,
        }
    }

    fn header(&mut self, out: &mut Vec<u8>) {
        if self.format == Format::Gzip && !self.started {
            // ID1 ID2 CM=8 FLG=0 MTIME=0 XFL=0 OS=255 (неизвестна).
            out.extend_from_slice(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255]);
        }
        self.started = true;
    }

    fn run(&mut self, mut input: &[u8], flush: TDEFLFlush, out: &mut Vec<u8>) -> Result<(), Fail> {
        let Some(engine) = self.engine.as_mut() else {
            return Err(Fail("invalid_state", "The compressor has finished".into()));
        };
        let mut buffer = first_buffer(input.len());
        loop {
            let (status, consumed, written) = compress(engine, input, &mut buffer, flush);
            out.extend_from_slice(&buffer[..written]);
            input = &input[consumed..];
            let filled = written == buffer.len();
            if filled {
                grown(&mut buffer);
            }
            match status {
                TDEFLStatus::Done => return Ok(()),
                TDEFLStatus::Okay => {
                    if flush == TDEFLFlush::None && input.is_empty() && !filled {
                        return Ok(());
                    }
                }
                _ => return Err(Fail("invalid_state", "The compressor failed".into())),
            }
        }
    }

    fn write(&mut self, input: &[u8]) -> Result<Vec<u8>, Fail> {
        if self.finished {
            return Err(Fail("invalid_state", "The compressor has finished".into()));
        }
        let mut out = Vec::new();
        self.header(&mut out);
        self.crc = crc32(self.crc, input);
        self.size = self.size.wrapping_add(input.len() as u32);
        self.run(input, TDEFLFlush::None, &mut out)?;
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<u8>, Fail> {
        if self.finished {
            return Err(Fail("invalid_state", "The compressor has finished".into()));
        }
        let mut out = Vec::new();
        self.header(&mut out);
        self.run(&[], TDEFLFlush::Finish, &mut out)?;
        if self.format == Format::Gzip {
            out.extend_from_slice(&self.crc.to_le_bytes());
            out.extend_from_slice(&self.size.to_le_bytes());
        }
        self.finished = true;
        if let Some(engine) = self.engine.take() {
            give_back(&DEFLATERS, engine);
        }
        Ok(out)
    }
}

impl Drop for Compressor {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            give_back(&DEFLATERS, engine);
        }
    }
}

/// Длина полного заголовка gzip в начале `bytes`, `None`, пока он не пришёл
/// целиком.
fn gzip_header(bytes: &[u8]) -> Result<Option<usize>, Fail> {
    let magic = [0x1f, 0x8b, 8];
    for (index, expected) in magic.iter().enumerate() {
        if bytes.get(index).is_some_and(|byte| byte != expected) {
            return Err(invalid("The data is not a gzip stream"));
        }
    }
    if bytes.len() < 10 {
        return Ok(None);
    }
    let flags = bytes[3];
    if flags & 0xe0 != 0 {
        return Err(invalid("A reserved gzip flag is set"));
    }
    let mut at = 10;
    if flags & 4 != 0 {
        if bytes.len() < at + 2 {
            return Ok(None);
        }
        at += 2 + u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
        if bytes.len() < at {
            return Ok(None);
        }
    }
    for flag in [8u8, 16] {
        if flags & flag != 0 {
            match bytes[at..].iter().position(|&byte| byte == 0) {
                Some(end) => at += end + 1,
                None => return Ok(None),
            }
        }
    }
    if flags & 2 != 0 {
        at += 2;
        if bytes.len() < at {
            return Ok(None);
        }
        // FHCRC: the low 16 bits of the CRC-32 of the header before it (#990).
        let stored = u16::from_le_bytes([bytes[at - 2], bytes[at - 1]]);
        if stored != crc32(0, &bytes[..at - 2]) as u16 {
            return Err(invalid("The gzip header checksum does not match"));
        }
    }
    // The limit holds for a complete header too, not only a partial one (#990).
    if at > HEADER_LIMIT {
        return Err(invalid("The gzip header is too long"));
    }
    Ok(Some(at))
}

struct Decompressor {
    /// Held until the stream ends, then given back to the pool.
    engine: Option<Box<InflateState>>,
    format: Format,
    stage: Stage,
    /// Начало заголовка или хвоста, пришедшее не целиком.
    held: Vec<u8>,
    crc: u32,
    size: u32,
    produced: u64,
    limit: u64,
    remainder: Vec<u8>,
}

impl Decompressor {
    fn new(format: Format, limit: u64) -> Self {
        let data = if format == Format::Zlib { DataFormat::Zlib } else { DataFormat::Raw };
        Self {
            engine: Some(inflater(data)),
            format,
            stage: if format == Format::Gzip { Stage::Header } else { Stage::Body },
            held: Vec::new(),
            crc: 0,
            size: 0,
            produced: 0,
            limit,
            remainder: Vec::new(),
        }
    }

    fn write(&mut self, source: &[u8]) -> Result<Vec<u8>, Fail> {
        let mut input = std::mem::take(&mut self.held);
        input.extend_from_slice(source);
        let mut out = Vec::new();
        let mut at = 0usize;
        let mut buffer = first_buffer(input.len().saturating_mul(4));
        loop {
            match self.stage {
                Stage::Ended => {
                    self.remainder.extend_from_slice(&input[at..]);
                    return Ok(out);
                }
                Stage::Header => match gzip_header(&input[at..])? {
                    Some(length) => {
                        at += length;
                        self.stage = Stage::Body;
                    }
                    None => {
                        if input.len() - at > HEADER_LIMIT {
                            return Err(invalid("The gzip header is too long"));
                        }
                        self.held = input[at..].to_vec();
                        return Ok(out);
                    }
                },
                Stage::Trailer => {
                    if input.len() - at < 8 {
                        self.held = input[at..].to_vec();
                        return Ok(out);
                    }
                    let tail = &input[at..at + 8];
                    let crc = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
                    let size = u32::from_le_bytes([tail[4], tail[5], tail[6], tail[7]]);
                    if crc != self.crc || size != self.size {
                        return Err(invalid("The gzip trailer does not match the data"));
                    }
                    at += 8;
                    self.stage = Stage::Ended;
                }
                Stage::Body => {
                    let Some(engine) = self.engine.as_mut() else {
                        return Err(invalid("The compressed stream has ended"));
                    };
                    let result = inflate(engine, &input[at..], &mut buffer, MZFlush::None);
                    at += result.bytes_consumed;
                    let produced = &buffer[..result.bytes_written];
                    self.produced += produced.len() as u64;
                    if self.produced > self.limit {
                        return Err(exceeded(self.limit));
                    }
                    if self.format == Format::Gzip {
                        self.crc = crc32(self.crc, produced);
                        self.size = self.size.wrapping_add(produced.len() as u32);
                    }
                    out.extend_from_slice(produced);
                    let filled = result.bytes_written == buffer.len();
                    match result.status {
                        Ok(MZStatus::StreamEnd) => {
                            self.stage = if self.format == Format::Gzip { Stage::Trailer } else { Stage::Ended };
                            if let Some(engine) = self.engine.take() {
                                give_back(&INFLATERS, engine);
                            }
                        }
                        Ok(_) | Err(MZError::Buf) => {
                            let stuck = result.bytes_consumed == 0 && result.bytes_written == 0;
                            if stuck || (at >= input.len() && !filled) {
                                return Ok(out);
                            }
                            if filled {
                                grown(&mut buffer);
                            }
                        }
                        Err(_) => return Err(invalid("The compressed data is damaged")),
                    }
                }
            }
        }
    }

    fn finish(&mut self) -> Result<Vec<u8>, Fail> {
        if self.stage != Stage::Ended {
            return Err(invalid("The compressed stream is truncated"));
        }
        Ok(Vec::new())
    }
}

impl Drop for Decompressor {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            give_back(&INFLATERS, engine);
        }
    }
}

enum Codec {
    Compress(Compressor),
    Decompress(Decompressor),
    Other(Box<dyn Stream>),
}

/// The levels a format takes: 0..9 for DEFLATE, 1..22 for zstd, 0..11 for
/// Brotli.
fn levels(format: &str) -> Option<(i64, i64)> {
    match format {
        "deflate" | "zlib" | "gzip" => Some((0, 9)),
        "zstd" => Some((1, 22)),
        "brotli" => Some((0, 11)),
        _ => None,
    }
}

/// A compressor's `limit` when the size of its whole input is not known.
const UNKNOWN_SIZE: u64 = u64::MAX;

/// A codec for `format`. Decompressing, `limit` caps the output; compressing,
/// it is the whole input's size, or [`UNKNOWN_SIZE`].
fn open(format: &str, compressing: bool, level: i64, limit: u64) -> Result<Codec, Fail> {
    let deflate = |format| {
        if compressing {
            Codec::Compress(Compressor::new(format, level as u8))
        } else {
            Codec::Decompress(Decompressor::new(format, limit))
        }
    };
    Ok(match (format, compressing) {
        ("deflate", _) => deflate(Format::Deflate),
        ("zlib", _) => deflate(Format::Zlib),
        ("gzip", _) => deflate(Format::Gzip),
        ("zstd", true) => {
            let size = (limit != UNKNOWN_SIZE).then_some(limit);
            Codec::Other(Box::new(zstd_codec::ZstdCompressor::new(level as i32, size)?))
        }
        ("zstd", false) => Codec::Other(Box::new(zstd_codec::ZstdDecompressor::new(limit)?)),
        ("brotli", true) => Codec::Other(Box::new(brotli_codec::BrotliCompressor::new(level as u32))),
        ("brotli", false) => Codec::Other(Box::new(brotli_codec::BrotliDecompressor::new(limit))),
        _ => return Err(invalid("Unknown compression format")),
    })
}

fn failure(native: Native, Fail(code, message): Fail) -> Error {
    Error::Failure(NativeFailure {
        domain: "core.compression".into(),
        code: code.into(),
        operation: native.symbol().into(),
        message,
    })
}

fn bytes(value: &Value, arrays: &ArrayHeap) -> Result<Vec<u8>, Error> {
    Ok(arrays
        .elements(value)?
        .iter()
        .map(Value::as_byte)
        .collect::<Result<Vec<_>, _>>()?)
}

fn state(value: &Value) -> Result<&RefCell<Codec>, Error> {
    super::support::handle::<RefCell<Codec>>(value)
}

fn array(arrays: &mut ArrayHeap, out: Vec<u8>) -> Value {
    arrays.allocate(out.into_iter().map(Value::Byte).collect())
}

pub fn handles(native: Native) -> bool {
    use Native::*;
    matches!(
        native,
        CompressionCodecOpen
            | CompressionCodecWrite
            | CompressionCodecFinish
            | CompressionCodecEnded
            | CompressionCodecRemainder
            | ChecksumChecksumCrc32
            | ChecksumChecksumCrc16
            | ChecksumChecksumAdler32
    )
}

pub fn execute(native: Native, args: &[Value], arrays: &mut ArrayHeap) -> Result<Value, Error> {
    use Native::*;
    match native {
        ChecksumChecksumCrc32 => {
            let initial = seed(native, &args[1], u32::MAX)?;
            Ok(super::support::int_value(i64::from(crc32(initial, &bytes(&args[0], arrays)?))))
        }
        ChecksumChecksumCrc16 => {
            let initial = seed(native, &args[1], 0xFFFF)? as u16;
            Ok(super::support::int_value(i64::from(crc16(initial, &bytes(&args[0], arrays)?))))
        }
        ChecksumChecksumAdler32 => {
            let initial = seed(native, &args[1], u32::MAX)?;
            Ok(super::support::int_value(i64::from(adler32(initial, &bytes(&args[0], arrays)?))))
        }
        CompressionCodecOpen => {
            let format = args[0].as_string()?;
            let Some((low, high)) = levels(format) else {
                return Err(failure(native, invalid("Unknown compression format")));
            };
            let compressing = args[1].as_bool()?;
            let (level, limit) = if compressing {
                let Some(level) = args[2].as_int()?.to_i64().filter(|level| (low..=high).contains(level)) else {
                    return Err(failure(native, invalid(format!("Compression level must be {low}..{high}"))));
                };
                // The whole input's size when known (#939); negative: unknown.
                let size = args[3].as_int()?.to_i64().and_then(|size| u64::try_from(size).ok());
                (level, size.unwrap_or(UNKNOWN_SIZE))
            } else {
                let Some(limit) = args[3].as_int()?.to_i64().filter(|limit| *limit > 0) else {
                    return Err(failure(native, invalid("Decompression limit must be positive")));
                };
                (0, limit as u64)
            };
            let codec = open(format, compressing, level, limit).map_err(|fail| failure(native, fail))?;
            Ok(Value::Handle(Rc::new(RefCell::new(codec))))
        }
        CompressionCodecWrite => {
            let input = bytes(&args[1], arrays)?;
            let out = match &mut *state(&args[0])?.borrow_mut() {
                Codec::Compress(codec) => codec.write(&input),
                Codec::Decompress(codec) => codec.write(&input),
                Codec::Other(codec) => codec.write(&input),
            }
            .map_err(|fail| failure(native, fail))?;
            Ok(array(arrays, out))
        }
        CompressionCodecFinish => {
            let out = match &mut *state(&args[0])?.borrow_mut() {
                Codec::Compress(codec) => codec.finish(),
                Codec::Decompress(codec) => codec.finish(),
                Codec::Other(codec) => codec.finish(),
            }
            .map_err(|fail| failure(native, fail))?;
            Ok(array(arrays, out))
        }
        CompressionCodecEnded => Ok(Value::Bool(match &*state(&args[0])?.borrow() {
            Codec::Compress(codec) => codec.finished,
            Codec::Decompress(codec) => codec.stage == Stage::Ended,
            Codec::Other(codec) => codec.ended(),
        })),
        CompressionCodecRemainder => {
            let out = match &*state(&args[0])?.borrow() {
                Codec::Compress(_) => Vec::new(),
                Codec::Decompress(codec) => codec.remainder.clone(),
                Codec::Other(codec) => codec.remainder(),
            };
            Ok(array(arrays, out))
        }
        _ => Err(Error::InvalidState("not a compression native".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(format: Format, data: &[u8], pieces: usize) -> Vec<u8> {
        let mut compressor = Compressor::new(format, 6);
        let mut packed = Vec::new();
        for chunk in data.chunks((data.len() / pieces).max(1)) {
            packed.extend(compressor.write(chunk).ok().expect("write"));
        }
        packed.extend(compressor.finish().ok().expect("finish"));
        let mut decompressor = Decompressor::new(format, 1 << 30);
        let mut plain = Vec::new();
        for chunk in packed.chunks(7) {
            plain.extend(decompressor.write(chunk).ok().expect("inflate"));
        }
        assert!(decompressor.finish().is_ok());
        plain
    }

    /// #990: FHCRC is compared, and a complete header past the limit is
    /// refused like a partial one.
    #[test]
    fn gzip_header_checks_its_crc_and_its_limit() {
        let mut header = vec![0x1f, 0x8b, 8, 2, 0, 0, 0, 0, 0, 3];
        let crc = crc32(0, &header) as u16;
        let mut good = header.clone();
        good.extend(crc.to_le_bytes());
        assert_eq!(gzip_header(&good).ok().flatten(), Some(12));
        header.extend((crc ^ 1).to_le_bytes());
        assert!(gzip_header(&header).is_err());

        let mut long = vec![0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 3];
        long.extend(std::iter::repeat_n(b'a', HEADER_LIMIT));
        long.push(0);
        assert!(gzip_header(&long).is_err());
        let mut short = vec![0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 3];
        short.extend(b"name\0");
        assert_eq!(gzip_header(&short).ok().flatten(), Some(15));
    }

    #[test]
    fn every_format_round_trips_in_pieces() {
        let data: Vec<u8> = (0..200_000u32).map(|n| (n % 251) as u8 ^ (n / 1000) as u8).collect();
        for format in [Format::Deflate, Format::Zlib, Format::Gzip] {
            assert_eq!(roundtrip(format, &data, 13), data);
            assert_eq!(roundtrip(format, b"", 1), b"");
        }
    }

    fn other_roundtrip(format: &str, level: i64, data: &[u8], pieces: usize) -> (Vec<u8>, usize) {
        let Ok(Codec::Other(mut compressor)) = open(format, true, level, UNKNOWN_SIZE) else { panic!("open") };
        let mut packed = Vec::new();
        for chunk in data.chunks((data.len() / pieces).max(1)) {
            packed.extend(compressor.write(chunk).ok().expect("write"));
        }
        packed.extend(compressor.finish().ok().expect("finish"));
        assert!(compressor.ended());
        assert!(compressor.finish().is_err());
        let size = packed.len();
        packed.extend_from_slice(b"rest");
        let Ok(Codec::Other(mut decompressor)) = open(format, false, 0, 1 << 30) else { panic!("open") };
        let mut plain = Vec::new();
        for chunk in packed.chunks(7) {
            plain.extend(decompressor.write(chunk).ok().expect("decompress"));
        }
        assert!(decompressor.finish().is_ok());
        assert!(decompressor.ended());
        assert_eq!(decompressor.remainder(), b"rest");
        (plain, size)
    }

    #[test]
    fn zstd_and_brotli_round_trip_and_keep_the_remainder() {
        let data: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8 ^ (n / 1000) as u8).collect();
        for (format, level) in [("zstd", 3), ("zstd", 19), ("brotli", 5), ("brotli", 11), ("brotli", 0)] {
            let (plain, size) = other_roundtrip(format, level, &data, 13);
            assert_eq!(plain, data, "{format} {level}");
            assert!(size * 4 < data.len(), "{format} {level}: {size}");
            assert_eq!(other_roundtrip(format, level, b"", 1).0, b"");
        }
    }

    #[test]
    fn zstd_and_brotli_refuse_damage_truncation_and_overflow() {
        let data = b"tea ".repeat(1000);
        for format in ["zstd", "brotli"] {
            let Ok(Codec::Other(mut compressor)) = open(format, true, levels(format).unwrap().0 + 1, UNKNOWN_SIZE) else {
                panic!("open")
            };
            let mut packed = compressor.write(&data).ok().unwrap();
            packed.extend(compressor.finish().ok().unwrap());
            let Ok(Codec::Other(mut small)) = open(format, false, 0, 100) else { panic!("open") };
            assert!(matches!(small.write(&packed), Err(Fail("resource_exhausted", _))), "{format}");
            let Ok(Codec::Other(mut cut)) = open(format, false, 0, 1 << 20) else { panic!("open") };
            assert!(cut.write(&packed[..packed.len() - 3]).is_ok());
            assert!(matches!(cut.finish(), Err(Fail("invalid_input", _))), "{format}");
            let Ok(Codec::Other(mut garbage)) = open(format, false, 0, 1 << 20) else { panic!("open") };
            let noise: Vec<u8> = (0..64u8).map(|n| n.wrapping_mul(37) ^ 0xA5).collect();
            let refused = garbage.write(&noise).is_err() || garbage.finish().is_err();
            assert!(refused, "{format}");
        }
        assert!(levels("zstd") == Some((1, 22)) && levels("brotli") == Some((0, 11)));
        assert!(open("lzma", true, 1, 1).is_err());
    }

    /// #939: a whole input of known size is compressed into a frame that says
    /// its size, reads back whole, and costs far less to start than a stream
    /// of unknown size.
    #[test]
    fn zstd_with_a_known_size_records_it_and_starts_cheaply() {
        let compress = |data: &[u8], size: u64| {
            let Ok(Codec::Other(mut compressor)) = open("zstd", true, 19, size) else { panic!("open") };
            let mut packed = compressor.write(data).ok().expect("write");
            packed.extend(compressor.finish().ok().expect("finish"));
            packed
        };
        for data in [&b"8 bytes!"[..], b"", &b"tea ".repeat(5000)] {
            let packed = compress(data, data.len() as u64);
            // Frame header descriptor: Frame_Content_Size_flag (bits 7-6) or
            // Single_Segment_flag (bit 5) set means the content size is present.
            assert!(packed[4] & 0b1110_0000 != 0, "{} bytes: no content size", data.len());
            let Ok(Codec::Other(mut decompressor)) = open("zstd", false, 0, 1 << 20) else { panic!("open") };
            assert_eq!(decompressor.write(&packed).ok().expect("decompress"), data);
            assert!(decompressor.finish().is_ok());
        }
        let unknown = compress(b"8 bytes!", UNKNOWN_SIZE);
        assert_eq!(unknown[4] & 0b1110_0000, 0, "an unknown size is not recorded");
        let time = |size: u64| {
            let started = std::time::Instant::now();
            for _ in 0..20 {
                compress(b"8 bytes!", size);
            }
            started.elapsed()
        };
        let (known, unknown) = (time(8), time(UNKNOWN_SIZE));
        assert!(known * 4 < unknown, "known {known:?} vs unknown {unknown:?}");
    }

    #[test]
    fn zstd_and_brotli_read_what_the_reference_tools_wrote() {
        // `printf 'hello, world' | zstd` (1.5.7) and `| brotli` (1.2.1), the reference CLIs.
        let zstd = [
            0x28, 0xb5, 0x2f, 0xfd, 0x04, 0x58, 0x61, 0x00, 0x00, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20, 0x77,
            0x6f, 0x72, 0x6c, 0x64, 0x42, 0x12, 0x1b, 0x6d,
        ];
        let brotli = [0x8f, 0x05, 0x80, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20, 0x77, 0x6f, 0x72, 0x6c, 0x64, 0x03];
        for (format, packed) in [("zstd", &zstd[..]), ("brotli", &brotli[..])] {
            let Ok(Codec::Other(mut codec)) = open(format, false, 0, 64) else { panic!("open") };
            assert_eq!(codec.write(packed).ok().unwrap(), b"hello, world", "{format}");
            assert!(codec.finish().is_ok());
        }
    }

    #[test]
    fn crc16_and_adler32_reference_checks() {
        assert_eq!(crc16(0xFFFF, b"123456789"), 10673);
        assert_eq!(crc16(0, b"123456789"), 12739);
        assert_eq!(adler32(1, b"Wikipedia"), 300286872);
        assert_eq!(adler32(1, &[0, 128, 255]), 33685888);
        assert_eq!(adler32(adler32(1, b"Wiki"), b"pedia"), 300286872);
        let long = vec![255u8; 100_000];
        let stepwise = long.iter().fold(1u32, |sum, byte| adler32(sum, &[*byte]));
        assert_eq!(adler32(1, &long), stepwise);
    }

    #[test]
    fn crc32_is_the_iso_hdlc_check() {
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(crc32(0, b"1234"), b"56789"), 0xCBF4_3926);
    }

    #[test]
    fn gzip_rejects_a_damaged_trailer_and_keeps_the_remainder() {
        let mut compressor = Compressor::new(Format::Gzip, 6);
        let mut packed = compressor.write(b"hello").ok().unwrap();
        packed.extend(compressor.finish().ok().unwrap());
        let mut tail = packed.clone();
        tail.extend_from_slice(b"rest");
        let mut decompressor = Decompressor::new(Format::Gzip, 100);
        assert_eq!(decompressor.write(&tail).ok().unwrap(), b"hello");
        assert_eq!(decompressor.remainder, b"rest");
        let last = packed.len() - 1;
        packed[last] ^= 1;
        let mut broken = Decompressor::new(Format::Gzip, 100);
        assert!(broken.write(&packed).is_err());
        let mut small = Decompressor::new(Format::Gzip, 3);
        assert!(small.write(&tail).is_err());
    }

    fn packed(compressor: &mut Compressor, data: &[u8]) -> Vec<u8> {
        let mut out = compressor.write(data).ok().expect("write");
        out.extend(compressor.finish().ok().expect("finish"));
        out
    }

    fn fresh(format: Format, level: u8) -> Compressor {
        let window = if format == Format::Zlib { 15 } else { -15 };
        let mut compressor = Compressor::new(format, level);
        let engine = Box::new(CompressorOxide::new(create_comp_flags_from_zip_params(level as i32, window, 0)));
        drop(compressor.engine.replace(engine));
        compressor
    }

    fn inputs() -> Vec<Vec<u8>> {
        let mut noise = Vec::new();
        let mut state = 7u32;
        for _ in 0..(CHUNK + 4099) {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            noise.push((state >> 16) as u8);
        }
        vec![Vec::new(), b"12345678".to_vec(), b"tea ".repeat(25_000), noise]
    }

    #[test]
    fn compression_pooled_engines_write_what_fresh_engines_write() {
        // #943: a reset engine from the pool must give byte-identical output,
        // even after it served another format and level.
        for format in [Format::Deflate, Format::Zlib, Format::Gzip] {
            for level in [0u8, 1, 6, 9] {
                for data in inputs() {
                    let mut dirty = Compressor::new(if format == Format::Zlib { Format::Gzip } else { Format::Zlib }, 9 - level);
                    let _ = packed(&mut dirty, b"warming the pooled engine with other settings");
                    let pooled = packed(&mut Compressor::new(format, level), &data);
                    let expected = packed(&mut fresh(format, level), &data);
                    assert!(pooled == expected, "format {} level {level} size {}", format as u8, data.len());
                    let mut back = Decompressor::new(format, 1 << 30);
                    let mut out = back.write(&pooled).ok().expect("inflate");
                    out.extend(back.finish().ok().expect("end"));
                    assert!(out == data);
                }
            }
        }
    }

    #[test]
    fn compression_finished_codecs_give_their_engines_back() {
        DEFLATERS.with(|pool| pool.borrow_mut().clear());
        INFLATERS.with(|pool| pool.borrow_mut().clear());
        let deflaters = || DEFLATERS.with(|pool| pool.borrow().len());
        let inflaters = || INFLATERS.with(|pool| pool.borrow().len());
        let mut compressor = Compressor::new(Format::Zlib, 6);
        let data = packed(&mut compressor, b"pool");
        assert_eq!(deflaters(), 1);
        let second = Compressor::new(Format::Zlib, 6);
        assert_eq!(deflaters(), 0);
        drop(second);
        assert_eq!(deflaters(), 1);
        let mut decompressor = Decompressor::new(Format::Zlib, 1024);
        assert!(decompressor.write(&data).ok().expect("inflate") == b"pool");
        assert_eq!(inflaters(), 1);
        let unfinished = Decompressor::new(Format::Zlib, 1024);
        assert_eq!(inflaters(), 0);
        drop(unfinished);
        assert_eq!(inflaters(), 1);
        let many: Vec<Compressor> = (0..POOLED + 3).map(|_| Compressor::new(Format::Deflate, 1)).collect();
        drop(many);
        assert_eq!(deflaters(), POOLED);
    }

    #[test]
    fn compression_small_buffers_grow_for_large_output() {
        // One write that expands far beyond the first buffer of a small input.
        let data = vec![0u8; 3 * CHUNK + 17];
        for format in [Format::Deflate, Format::Zlib, Format::Gzip] {
            let compressed = packed(&mut Compressor::new(format, 9), &data);
            assert!(compressed.len() < 4096);
            let mut back = Decompressor::new(format, 1 << 30);
            let mut out = back.write(&compressed).ok().expect("inflate");
            out.extend(back.finish().ok().expect("end"));
            assert!(out == data);
        }
    }
}

