use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::style::Color;
use tempfile::{Builder as TempFileBuilder, NamedTempFile};

use crate::pixel::{CoverDetail, CoverPalette, PixelCell, PixelCover};

/// Finished renders are ~14KB each, so this holds a few thousand — years
/// of terminal sizes and themes — while staying bounded.
const PIXEL_LIMIT_BYTES: u64 = 32 * 1024 * 1024;

/// The covers' slice of the shared cache budget. Estimated from what the
/// layers actually store: a cover original runs ~675KB against ~27MB for a
/// lossless track (~2.5% per song), widened for browsing — previews fetch
/// covers for songs never played — and for lossy tracks, which shift the
/// per-song ratio up. The floor keeps small budgets useful; the ceiling
/// stops terabyte budgets from hoarding gigabytes of artwork.
const COVER_BUDGET_PERCENT: u64 = 4;
const COVER_BUDGET_FLOOR: u64 = 256 * 1024 * 1024;
const COVER_BUDGET_CEILING: u64 = 2 * 1024 * 1024 * 1024;

/// The covers' share of `total_cache_bytes`, the user's whole cache budget.
pub fn cover_budget(total_cache_bytes: u64) -> u64 {
    (total_cache_bytes / 100 * COVER_BUDGET_PERCENT).clamp(COVER_BUDGET_FLOOR, COVER_BUDGET_CEILING)
}

/// What the audio store keeps once the covers take their slice. Saturating
/// because the total may come from the shared database, which other writers
/// size without knowing about the covers.
pub fn audio_share(total_cache_bytes: u64) -> u64 {
    total_cache_bytes.saturating_sub(cover_budget(total_cache_bytes))
}
const ORIGINAL_MAGIC: &[u8; 8] = b"YPMCOVO1";
const PIXEL_MAGIC: &[u8; 8] = b"YPMCOVP2";
const ORIGINAL_HEADER_LEN: usize = 8 + 4 + 8 + 8;
const PIXEL_HEADER_LEN: usize = 8 + 4 + 2 + 2 + 4 + 8 + 8;
const COLOR_BYTES: usize = 4;
const GLYPH_BYTES: usize = 4;
const CELL_BYTES: usize = GLYPH_BYTES + COLOR_BYTES * 2;
const MAX_KEY_BYTES: usize = 16 * 1024;
const MAX_PIXEL_CELLS: usize = 4096;
const MAX_PIXEL_FILE_BYTES: u64 =
    PIXEL_HEADER_LEN as u64 + MAX_KEY_BYTES as u64 + (MAX_PIXEL_CELLS * CELL_BYTES) as u64;
const TEMP_PREFIX: &str = ".ypm-cover-";
const TEMP_SUFFIX: &str = ".tmp";

const PIXEL_ALGORITHM_REVISION: u32 = 3;

#[derive(Debug)]
pub struct CoverCache {
    root: PathBuf,
    /// Atomic so a settings change retargets the running instance instead
    /// of waiting for the next launch.
    original_limit: AtomicU64,
    pixel_limit: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct PixelKeyInputs<'a> {
    pub(crate) song_id: i64,
    pub(crate) original_key: &'a str,
    pub(crate) cells: (u16, u16),
    pub(crate) detail_scale: f32,
    pub(crate) detail: CoverDetail,
    pub(crate) palette_mode: CoverPalette,
    pub(crate) background: Color,
    pub(crate) palette: &'a [(u8, u8, u8)],
}

impl CoverCache {
    /// `budget` is the covers' slice of the shared cache budget (see
    /// [`cover_budget`]); the pixel layer's fixed cut comes out of it and
    /// the originals keep the rest.
    pub fn new(root: impl Into<PathBuf>, budget: u64) -> io::Result<Self> {
        // Config parsing rejects totals whose cover slice could dip under
        // the floor, so the budget always clears the pixel cut.
        Self::new_with_limits(root.into(), budget - PIXEL_LIMIT_BYTES, PIXEL_LIMIT_BYTES)
    }

    #[cfg(test)]
    fn with_limit(root: impl Into<PathBuf>, original_limit: u64) -> io::Result<Self> {
        Self::new_with_limits(root.into(), original_limit, PIXEL_LIMIT_BYTES)
    }

    #[cfg(test)]
    fn with_limits(
        root: impl Into<PathBuf>,
        original_limit: u64,
        pixel_limit: u64,
    ) -> io::Result<Self> {
        Self::new_with_limits(root.into(), original_limit, pixel_limit)
    }

    fn new_with_limits(root: PathBuf, original_limit: u64, pixel_limit: u64) -> io::Result<Self> {
        fs::create_dir_all(root.join("original"))?;
        fs::create_dir_all(root.join("pixel"))?;
        let cache = Self {
            root,
            original_limit: AtomicU64::new(original_limit),
            pixel_limit,
        };
        cache.with_lock(|| {
            cache.clean_temporary_files();
            cache.trim_originals()?;
            cache.trim_pixels()?;
            Ok(())
        })?;
        Ok(cache)
    }

    pub fn original_key(pic_url: &str, edge: u32) -> String {
        format!("original-v1|edge={edge}|url={}:{}", pic_url.len(), pic_url)
    }

    pub fn pixel_key(inputs: PixelKeyInputs<'_>) -> String {
        pixel_key_with_revision(PIXEL_ALGORITHM_REVISION, inputs)
    }

    pub fn get_original(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
        validate_key(key)?;
        self.with_lock(|| {
            let path = self.original_path(key);
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            let file_len = file.metadata()?.len();
            if file_len > self.original_limit.load(Ordering::Relaxed) {
                drop(file);
                remove_invalid(&path);
                return Ok(None);
            }
            let mut encoded = Vec::with_capacity(usize::try_from(file_len).unwrap_or(0));
            file.read_to_end(&mut encoded)?;
            let Some(payload) = decode_original(&encoded, key) else {
                drop(file);
                remove_invalid(&path);
                return Ok(None);
            };
            drop(file);
            touch(&path);
            Ok(Some(payload))
        })
    }

    pub fn put_original(&self, key: &str, bytes: &[u8]) -> io::Result<()> {
        validate_key(key)?;
        let key_len =
            u32::try_from(key.len()).map_err(|_| invalid_input("cover key is too large"))?;
        let payload_len =
            u64::try_from(bytes.len()).map_err(|_| invalid_input("original cover is too large"))?;
        let encoded_len = (ORIGINAL_HEADER_LEN as u64)
            .checked_add(u64::from(key_len))
            .and_then(|length| length.checked_add(payload_len))
            .ok_or_else(|| invalid_input("original cover is too large"))?;
        if encoded_len > self.original_limit.load(Ordering::Relaxed) {
            return Ok(());
        }
        self.with_lock(|| {
            let path = self.original_path(key);
            if self.valid_original_at(&path, key)? {
                return Ok(());
            }
            remove_invalid(&path);

            let mut temporary = temporary_in(&self.original_dir())?;
            temporary.write_all(ORIGINAL_MAGIC)?;
            temporary.write_all(&key_len.to_le_bytes())?;
            temporary.write_all(&payload_len.to_le_bytes())?;
            temporary.write_all(&fnv1a(bytes).to_le_bytes())?;
            temporary.write_all(key.as_bytes())?;
            temporary.write_all(bytes)?;
            finish_temporary(temporary, &path)?;
            sync_directory(&self.original_dir())?;
            self.trim_originals()
        })
    }

    pub fn get_pixel(&self, key: &str) -> io::Result<Option<PixelCover>> {
        validate_key(key)?;
        self.with_lock(|| {
            let path = self.pixel_path(key);
            if fs::metadata(&path)
                .map(|metadata| metadata.len() > MAX_PIXEL_FILE_BYTES)
                .unwrap_or(false)
            {
                remove_invalid(&path);
                return Ok(None);
            }
            let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            let mut encoded = Vec::new();
            file.read_to_end(&mut encoded)?;
            let Some(cover) = decode_pixel(&encoded, key) else {
                drop(file);
                remove_invalid(&path);
                return Ok(None);
            };
            // Refresh the entry so eviction is least-recently-USED: a render
            // read every day must outlive one written yesterday and never
            // looked at again.
            let _ = file.set_modified(SystemTime::now());
            Ok(Some(cover))
        })
    }

    pub fn put_pixel(&self, key: &str, cover: &PixelCover) -> io::Result<()> {
        validate_key(key)?;
        let key_len =
            u32::try_from(key.len()).map_err(|_| invalid_input("cover key is too large"))?;
        let expected_cells = usize::from(cover.width)
            .checked_mul(usize::from(cover.height))
            .ok_or_else(|| invalid_input("pixel cover dimensions overflow"))?;
        if cover.width == 0 || cover.height == 0 || expected_cells != cover.cells.len() {
            return Err(invalid_input("pixel cover dimensions do not match cells"));
        }
        if expected_cells > MAX_PIXEL_CELLS {
            return Err(invalid_input("pixel cover is too large"));
        }

        let mut payload = Vec::with_capacity(expected_cells * CELL_BYTES);
        for cell in &cover.cells {
            payload.extend_from_slice(&(cell.glyph as u32).to_le_bytes());
            encode_color(&mut payload, cell.fg)?;
            encode_color(&mut payload, cell.bg)?;
        }
        let cell_count = u32::try_from(expected_cells)
            .map_err(|_| invalid_input("pixel cover has too many cells"))?;
        let payload_len = u64::try_from(payload.len())
            .map_err(|_| invalid_input("pixel cover payload is too large"))?;

        self.with_lock(|| {
            let path = self.pixel_path(key);
            if self.valid_pixel_at(&path, key)? {
                return Ok(());
            }
            remove_invalid(&path);

            let mut temporary = temporary_in(&self.pixel_dir())?;
            temporary.write_all(PIXEL_MAGIC)?;
            temporary.write_all(&key_len.to_le_bytes())?;
            temporary.write_all(&cover.width.to_le_bytes())?;
            temporary.write_all(&cover.height.to_le_bytes())?;
            temporary.write_all(&cell_count.to_le_bytes())?;
            temporary.write_all(&payload_len.to_le_bytes())?;
            temporary.write_all(
                &pixel_checksum(key, cover.width, cover.height, &payload).to_le_bytes(),
            )?;
            temporary.write_all(key.as_bytes())?;
            temporary.write_all(&payload)?;
            finish_temporary(temporary, &path)?;
            sync_directory(&self.pixel_dir())?;
            self.trim_pixels()
        })
    }

    fn valid_original_at(&self, path: &Path, key: &str) -> io::Result<bool> {
        if fs::metadata(path)
            .map(|metadata| metadata.len() > self.original_limit.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            return Ok(false);
        }
        match fs::read(path) {
            Ok(encoded) => Ok(decode_original(&encoded, key).is_some()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn valid_pixel_at(&self, path: &Path, key: &str) -> io::Result<bool> {
        if fs::metadata(path)
            .map(|metadata| metadata.len() > MAX_PIXEL_FILE_BYTES)
            .unwrap_or(false)
        {
            return Ok(false);
        }
        match fs::read(path) {
            Ok(encoded) => Ok(decode_pixel(&encoded, key).is_some()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Bytes the cache holds on disk right now, across both layers. Reads
    /// the directory without the lock: the count is a settings-page display,
    /// and a file landing mid-scan only shifts it by one entry. A directory
    /// that cannot be listed is an error — reporting it as 0 bytes would be
    /// indistinguishable from an empty cache.
    pub fn used_bytes(&self) -> io::Result<u64> {
        let mut total = 0;
        for dir in [self.original_dir(), self.pixel_dir()] {
            for entry in fs::read_dir(dir)? {
                let Ok(entry) = entry else { continue };
                if entry.path().extension().and_then(|e| e.to_str()) != Some("bin") {
                    continue;
                }
                if let Ok(metadata) = entry.metadata() {
                    total += metadata.len();
                }
            }
        }
        Ok(total)
    }

    pub fn budget_bytes(&self) -> u64 {
        self.original_limit.load(Ordering::Relaxed) + self.pixel_limit
    }

    /// Retarget the running instance when the user changes the total cache
    /// budget, trimming down right away if the new slice is smaller.
    pub fn set_budget(&self, budget: u64) -> io::Result<()> {
        self.original_limit
            .store(budget - PIXEL_LIMIT_BYTES, Ordering::Relaxed);
        self.with_lock(|| self.trim_originals())
    }

    fn trim_originals(&self) -> io::Result<()> {
        Self::trim_directory(
            &self.original_dir(),
            self.original_limit.load(Ordering::Relaxed),
        )
    }

    fn trim_pixels(&self) -> io::Result<()> {
        Self::trim_directory(&self.pixel_dir(), self.pixel_limit)
    }

    /// Oldest-mtime-first eviction down to `limit`. Reads refresh mtime, so
    /// this is least-recently-used, not first-in-first-out.
    fn trim_directory(directory: &Path, limit: u64) -> io::Result<()> {
        let mut entries = Vec::new();
        let mut total = 0_u64;
        for entry in fs::read_dir(directory)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("bin") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let size = metadata.len();
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            total = total.saturating_add(size);
            entries.push((modified, entry.file_name(), size, path));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        for (_, _, size, path) in entries {
            if total <= limit {
                break;
            }
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
        Ok(())
    }

    fn clean_temporary_files(&self) {
        for directory in [self.original_dir(), self.pixel_dir()] {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    fn with_lock<T>(&self, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("cache.lock"))?;
        File::lock(&lock)?;
        let result = operation();
        let unlock_result = File::unlock(&lock);
        match result {
            Ok(value) => {
                unlock_result?;
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }

    fn original_dir(&self) -> PathBuf {
        self.root.join("original")
    }

    fn pixel_dir(&self) -> PathBuf {
        self.root.join("pixel")
    }

    fn original_path(&self, key: &str) -> PathBuf {
        cache_path(&self.original_dir(), key)
    }

    fn pixel_path(&self, key: &str) -> PathBuf {
        cache_path(&self.pixel_dir(), key)
    }
}

fn pixel_key_with_revision(revision: u32, inputs: PixelKeyInputs<'_>) -> String {
    let PixelKeyInputs {
        song_id,
        original_key,
        cells,
        detail_scale,
        detail,
        palette_mode,
        background,
        palette,
    } = inputs;
    let mut key = format!(
        "pixel-v2|algorithm={revision}|song={song_id}|original={}:{}|cells={}x{}|scale={:08x}|detail={}|background=",
        original_key.len(),
        original_key,
        cells.0,
        cells.1,
        detail_scale.to_bits(),
        detail.as_str()
    );
    append_color_key(&mut key, background);
    match palette_mode {
        CoverPalette::Original => key.push_str("|palette=original"),
        CoverPalette::Theme => {
            write!(&mut key, "|palette={}", palette.len()).expect("writing to String cannot fail");
            for &(red, green, blue) in palette {
                write!(&mut key, ":{red:02x}{green:02x}{blue:02x}")
                    .expect("writing to String cannot fail");
            }
        }
    }
    key
}

fn append_color_key(key: &mut String, color: Color) {
    match color {
        Color::Reset => key.push_str("reset"),
        Color::Black => key.push_str("ansi:0"),
        Color::Red => key.push_str("ansi:1"),
        Color::Green => key.push_str("ansi:2"),
        Color::Yellow => key.push_str("ansi:3"),
        Color::Blue => key.push_str("ansi:4"),
        Color::Magenta => key.push_str("ansi:5"),
        Color::Cyan => key.push_str("ansi:6"),
        Color::Gray => key.push_str("ansi:7"),
        Color::DarkGray => key.push_str("ansi:8"),
        Color::LightRed => key.push_str("ansi:9"),
        Color::LightGreen => key.push_str("ansi:10"),
        Color::LightYellow => key.push_str("ansi:11"),
        Color::LightBlue => key.push_str("ansi:12"),
        Color::LightMagenta => key.push_str("ansi:13"),
        Color::LightCyan => key.push_str("ansi:14"),
        Color::White => key.push_str("ansi:15"),
        Color::Rgb(red, green, blue) => {
            write!(key, "rgb:{red:02x}{green:02x}{blue:02x}")
                .expect("writing to String cannot fail");
        }
        Color::Indexed(index) => {
            write!(key, "indexed:{index:02x}").expect("writing to String cannot fail");
        }
    }
}

fn encode_color(encoded: &mut Vec<u8>, color: Color) -> io::Result<()> {
    match color {
        Color::Reset => encoded.extend_from_slice(&[0, 0, 0, 0]),
        Color::Rgb(red, green, blue) => encoded.extend_from_slice(&[1, red, green, blue]),
        _ => {
            return Err(invalid_input(
                "pixel cache only supports Reset and Rgb colors",
            ))
        }
    }
    Ok(())
}

fn decode_original(encoded: &[u8], expected_key: &str) -> Option<Vec<u8>> {
    let mut input = encoded;
    if take_array::<8>(&mut input)? != *ORIGINAL_MAGIC {
        return None;
    }
    let key_len = u32::from_le_bytes(take_array::<4>(&mut input)?) as usize;
    let payload_len = usize::try_from(u64::from_le_bytes(take_array::<8>(&mut input)?)).ok()?;
    let checksum = u64::from_le_bytes(take_array::<8>(&mut input)?);
    if key_len > MAX_KEY_BYTES
        || encoded.len()
            != ORIGINAL_HEADER_LEN
                .checked_add(key_len)?
                .checked_add(payload_len)?
    {
        return None;
    }
    if take_bytes(&mut input, key_len)? != expected_key.as_bytes() {
        return None;
    }
    let payload = take_bytes(&mut input, payload_len)?;
    if !input.is_empty() || fnv1a(payload) != checksum {
        return None;
    }
    Some(payload.to_vec())
}

fn decode_pixel(encoded: &[u8], expected_key: &str) -> Option<PixelCover> {
    let mut input = encoded;
    if take_array::<8>(&mut input)? != *PIXEL_MAGIC {
        return None;
    }
    let key_len = u32::from_le_bytes(take_array::<4>(&mut input)?) as usize;
    let width = u16::from_le_bytes(take_array::<2>(&mut input)?);
    let height = u16::from_le_bytes(take_array::<2>(&mut input)?);
    let cell_count = u32::from_le_bytes(take_array::<4>(&mut input)?) as usize;
    let payload_len = usize::try_from(u64::from_le_bytes(take_array::<8>(&mut input)?)).ok()?;
    let checksum = u64::from_le_bytes(take_array::<8>(&mut input)?);
    let expected_cells = usize::from(width).checked_mul(usize::from(height))?;
    let expected_payload = expected_cells.checked_mul(CELL_BYTES)?;
    if width == 0
        || height == 0
        || expected_cells > MAX_PIXEL_CELLS
        || cell_count != expected_cells
        || payload_len != expected_payload
        || key_len > MAX_KEY_BYTES
        || encoded.len()
            != PIXEL_HEADER_LEN
                .checked_add(key_len)?
                .checked_add(payload_len)?
    {
        return None;
    }
    if take_bytes(&mut input, key_len)? != expected_key.as_bytes() {
        return None;
    }
    let payload = take_bytes(&mut input, payload_len)?;
    if !input.is_empty() || pixel_checksum(expected_key, width, height, payload) != checksum {
        return None;
    }

    let mut cells = Vec::with_capacity(expected_cells);
    for encoded_cell in payload.chunks_exact(CELL_BYTES) {
        let glyph = char::from_u32(u32::from_le_bytes(
            encoded_cell[..GLYPH_BYTES].try_into().ok()?,
        ))?;
        cells.push(PixelCell {
            glyph,
            fg: decode_color(&encoded_cell[GLYPH_BYTES..GLYPH_BYTES + COLOR_BYTES])?,
            bg: decode_color(&encoded_cell[GLYPH_BYTES + COLOR_BYTES..])?,
        });
    }
    Some(PixelCover {
        width,
        height,
        cells,
    })
}

fn decode_color(encoded: &[u8]) -> Option<Color> {
    match encoded {
        [0, 0, 0, 0] => Some(Color::Reset),
        [1, red, green, blue] => Some(Color::Rgb(*red, *green, *blue)),
        _ => None,
    }
}

fn take_array<const N: usize>(input: &mut &[u8]) -> Option<[u8; N]> {
    let head = input.get(..N)?;
    *input = input.get(N..)?;
    head.try_into().ok()
}

fn take_bytes<'a>(input: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    let head = input.get(..len)?;
    *input = input.get(len..)?;
    Some(head)
}

fn validate_key(key: &str) -> io::Result<()> {
    if key.len() > MAX_KEY_BYTES {
        return Err(invalid_input("cover cache key is too large"));
    }
    Ok(())
}

fn cache_path(directory: &Path, key: &str) -> PathBuf {
    directory.join(format!("{:016x}.bin", fnv1a(key.as_bytes())))
}

fn temporary_in(directory: &Path) -> io::Result<NamedTempFile> {
    TempFileBuilder::new()
        .prefix(TEMP_PREFIX)
        .suffix(TEMP_SUFFIX)
        .tempfile_in(directory)
}

fn finish_temporary(temporary: NamedTempFile, path: &Path) -> io::Result<()> {
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) => Err(error.error),
    }
}

/// Best-effort read-marker: mtime drives LRU order, so a hit refreshes it.
/// Reads stay read-only — on a read-only cache the refresh silently skips
/// and the hit still counts.
fn touch(path: &Path) {
    let _ = OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(SystemTime::now()));
}

fn remove_invalid(path: &Path) {
    let _ = fs::remove_file(path);
}

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    fnv1a_update(0xcbf2_9ce4_8422_2325_u64, bytes)
}

fn fnv1a_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn pixel_checksum(key: &str, width: u16, height: u16, payload: &[u8]) -> u64 {
    let hash = fnv1a_update(0xcbf2_9ce4_8422_2325_u64, key.as_bytes());
    let hash = fnv1a_update(hash, &width.to_le_bytes());
    let hash = fnv1a_update(hash, &height.to_le_bytes());
    fnv1a_update(hash, payload)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::time::{Duration, SystemTime};

    use ratatui::style::Color;
    use tempfile::tempdir;

    use super::*;

    fn sample_cover() -> PixelCover {
        PixelCover {
            width: 2,
            height: 1,
            cells: vec![
                PixelCell {
                    glyph: '\u{1fb00}',
                    fg: Color::Rgb(1, 2, 3),
                    bg: Color::Reset,
                },
                PixelCell {
                    glyph: '▄',
                    fg: Color::Rgb(4, 5, 6),
                    bg: Color::Reset,
                },
            ],
        }
    }

    #[test]
    fn original_key_includes_url_and_edge() {
        let key = CoverCache::original_key("https://example.test/cover", 512);
        assert_eq!(
            key,
            CoverCache::original_key("https://example.test/cover", 512)
        );
        assert_ne!(
            key,
            CoverCache::original_key("https://example.test/other", 512)
        );
        assert_ne!(
            key,
            CoverCache::original_key("https://example.test/cover", 1024)
        );
    }

    #[test]
    fn pixel_key_includes_every_rendering_input() {
        let palette = [(1, 2, 3), (4, 5, 6)];
        let reordered = [(4, 5, 6), (1, 2, 3)];
        let changed_palette = [(1, 2, 3), (4, 5, 7)];
        let inputs = PixelKeyInputs {
            song_id: 42,
            original_key: "source",
            cells: (26, 13),
            detail_scale: 1.0,
            detail: CoverDetail::Half,
            palette_mode: CoverPalette::Theme,
            background: Color::Reset,
            palette: &palette,
        };
        let key = pixel_key_with_revision(7, inputs);
        let changed = [
            pixel_key_with_revision(8, inputs),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    song_id: 43,
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    original_key: "other",
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    cells: (25, 13),
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    cells: (26, 12),
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    detail_scale: f32::from_bits(1.0_f32.to_bits() + 1),
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    detail: CoverDetail::Quad,
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    detail: CoverDetail::Sextant,
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    detail: CoverDetail::Octant,
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    palette_mode: CoverPalette::Original,
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    background: Color::Rgb(0, 0, 0),
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    palette: &reordered,
                    ..inputs
                },
            ),
            pixel_key_with_revision(
                7,
                PixelKeyInputs {
                    palette: &changed_palette,
                    ..inputs
                },
            ),
        ];
        assert!(changed.iter().all(|changed| changed != &key));
    }

    #[test]
    fn original_palette_key_is_theme_palette_independent() {
        let first_palette = [(1, 2, 3), (4, 5, 6)];
        let second_palette = [(9, 8, 7)];
        let inputs = PixelKeyInputs {
            song_id: 42,
            original_key: "source",
            cells: (26, 13),
            detail_scale: 1.0,
            detail: CoverDetail::Half,
            palette_mode: CoverPalette::Original,
            background: Color::Reset,
            palette: &first_palette,
        };

        let key = pixel_key_with_revision(PIXEL_ALGORITHM_REVISION, inputs);
        let changed_theme = pixel_key_with_revision(
            PIXEL_ALGORITHM_REVISION,
            PixelKeyInputs {
                palette: &second_palette,
                ..inputs
            },
        );

        assert_eq!(key, changed_theme);
        assert!(key.ends_with("|palette=original"));
    }

    #[test]
    fn theme_palette_key_tracks_the_current_algorithm_and_palette() {
        let palette = [(1, 2, 3), (4, 5, 6)];
        let key = pixel_key_with_revision(
            PIXEL_ALGORITHM_REVISION,
            PixelKeyInputs {
                song_id: 42,
                original_key: "source",
                cells: (26, 13),
                detail_scale: 1.0,
                detail: CoverDetail::Half,
                palette_mode: CoverPalette::Theme,
                background: Color::Reset,
                palette: &palette,
            },
        );

        assert!(key.contains("|algorithm=3|"));
        assert!(key.ends_with("|palette=2:010203:040506"));
        assert!(!key.contains("palette=theme"));
    }

    #[test]
    fn original_round_trip_touches_mtime() {
        let directory = tempdir().unwrap();
        let cache = CoverCache::with_limit(directory.path(), 1024).unwrap();
        let key = CoverCache::original_key("https://example.test/cover", 512);
        cache.put_original(&key, b"image bytes").unwrap();
        let path = cache.original_path(&key);
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();

        assert_eq!(
            cache.get_original(&key).unwrap(),
            Some(b"image bytes".to_vec())
        );
        assert!(fs::metadata(path).unwrap().modified().unwrap() > old);
    }

    #[test]
    fn corrupt_original_is_a_deleted_miss() {
        let directory = tempdir().unwrap();
        let cache = CoverCache::with_limit(directory.path(), 1024).unwrap();
        let key = CoverCache::original_key("https://example.test/cover", 512);
        cache.put_original(&key, b"image bytes").unwrap();
        let path = cache.original_path(&key);
        let mut encoded = fs::read(&path).unwrap();
        *encoded.last_mut().unwrap() ^= 1;
        fs::write(&path, encoded).unwrap();

        assert_eq!(cache.get_original(&key).unwrap(), None);
        assert!(!path.exists());
    }

    #[test]
    fn wrong_exact_key_is_a_deleted_miss() {
        let directory = tempdir().unwrap();
        let cache = CoverCache::with_limit(directory.path(), 1024).unwrap();
        let stored_key = "stored";
        let requested_key = "requested";
        cache.put_original(stored_key, b"image bytes").unwrap();
        let requested_path = cache.original_path(requested_key);
        fs::copy(cache.original_path(stored_key), &requested_path).unwrap();

        assert_eq!(cache.get_original(requested_key).unwrap(), None);
        assert!(!requested_path.exists());
    }

    #[test]
    fn pixel_round_trip_preserves_reset_and_rgb() {
        let directory = tempdir().unwrap();
        let cache = CoverCache::with_limit(directory.path(), 1024).unwrap();
        let cover = sample_cover();
        let key = "pixel-key";

        cache.put_pixel(key, &cover).unwrap();
        assert_eq!(cache.get_pixel(key).unwrap(), Some(cover));
    }

    #[test]
    fn corrupt_pixel_is_a_deleted_miss() {
        let directory = tempdir().unwrap();
        let cache = CoverCache::with_limit(directory.path(), 1024).unwrap();
        let key = "pixel-key";
        cache.put_pixel(key, &sample_cover()).unwrap();
        let path = cache.pixel_path(key);
        let mut encoded = fs::read(&path).unwrap();
        encoded.pop();
        fs::write(&path, encoded).unwrap();

        assert_eq!(cache.get_pixel(key).unwrap(), None);
        assert!(!path.exists());
    }

    #[test]
    fn pixel_checksum_covers_dimensions() {
        let directory = tempdir().unwrap();
        let cache = CoverCache::with_limit(directory.path(), 1024).unwrap();
        let key = "pixel-key";
        cache.put_pixel(key, &sample_cover()).unwrap();
        let path = cache.pixel_path(key);
        let mut encoded = fs::read(&path).unwrap();
        encoded.swap(12, 14);
        encoded.swap(13, 15);
        fs::write(&path, encoded).unwrap();

        assert_eq!(cache.get_pixel(key).unwrap(), None);
        assert!(!path.exists());
    }

    #[test]
    fn original_lru_evicts_the_oldest_file() {
        let directory = tempdir().unwrap();
        let entry_size = (ORIGINAL_HEADER_LEN + 1 + 4) as u64;
        let cache = CoverCache::with_limit(directory.path(), entry_size * 2).unwrap();
        cache.put_original("a", b"aaaa").unwrap();
        cache.put_original("b", b"bbbb").unwrap();
        OpenOptions::new()
            .write(true)
            .open(cache.original_path("a"))
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();
        OpenOptions::new()
            .write(true)
            .open(cache.original_path("b"))
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();

        cache.put_original("c", b"cccc").unwrap();

        assert!(!cache.original_path("a").exists());
        assert!(cache.original_path("b").exists());
        assert!(cache.original_path("c").exists());
    }

    /// One published pixel entry's size on disk, for sizing tight limits.
    fn pixel_entry_size(cache: &CoverCache, key: &str) -> u64 {
        fs::metadata(cache.pixel_path(key)).unwrap().len()
    }

    fn backdate(path: &std::path::Path, seconds: u64) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn pixel_lru_evicts_the_oldest_render() {
        let directory = tempdir().unwrap();
        let probe = CoverCache::with_limit(directory.path(), 1024).unwrap();
        probe.put_pixel("a", &sample_cover()).unwrap();
        let entry = pixel_entry_size(&probe, "a");
        // Re-open with room for exactly two renders.
        let cache = CoverCache::with_limits(directory.path(), 1024, entry * 2).unwrap();
        cache.put_pixel("b", &sample_cover()).unwrap();
        backdate(&cache.pixel_path("a"), 10);
        backdate(&cache.pixel_path("b"), 20);

        cache.put_pixel("c", &sample_cover()).unwrap();

        assert!(!cache.pixel_path("a").exists());
        assert!(cache.pixel_path("b").exists());
        assert!(cache.pixel_path("c").exists());
    }

    #[test]
    fn a_pixel_read_saves_the_entry_from_eviction() {
        let directory = tempdir().unwrap();
        let probe = CoverCache::with_limit(directory.path(), 1024).unwrap();
        probe.put_pixel("a", &sample_cover()).unwrap();
        let entry = pixel_entry_size(&probe, "a");
        let cache = CoverCache::with_limits(directory.path(), 1024, entry * 2).unwrap();
        cache.put_pixel("b", &sample_cover()).unwrap();
        backdate(&cache.pixel_path("a"), 10);
        backdate(&cache.pixel_path("b"), 20);

        // Reading "a" marks it used, so the trim drops "b" instead.
        assert!(cache.get_pixel("a").unwrap().is_some());
        cache.put_pixel("c", &sample_cover()).unwrap();

        assert!(cache.pixel_path("a").exists());
        assert!(!cache.pixel_path("b").exists());
        assert!(cache.pixel_path("c").exists());
    }

    #[test]
    fn a_budget_change_retargets_and_trims_the_running_instance() {
        let directory = tempdir().unwrap();
        let probe = CoverCache::with_limit(directory.path(), 1024).unwrap();
        probe.put_pixel("a", &sample_cover()).unwrap();
        let entry = pixel_entry_size(&probe, "a");
        let cache = CoverCache::with_limits(directory.path(), 1024, entry * 3).unwrap();
        cache.put_pixel("b", &sample_cover()).unwrap();
        cache.put_original("keep", b"kkkk").unwrap();
        backdate(&cache.pixel_path("a"), 10);
        backdate(&cache.pixel_path("b"), 20);

        // Shrink the budget below the originals' current bytes: the running
        // instance must trim now, not on the next launch.
        let original = fs::metadata(cache.original_path("keep")).unwrap().len();
        cache.set_budget(PIXEL_LIMIT_BYTES + original - 1).unwrap();
        assert!(!cache.original_path("keep").exists());
    }

    #[test]
    fn cover_budget_is_a_clamped_slice_of_the_whole_cache() {
        // 4% with a floor and a ceiling: small budgets stay usable, huge
        // budgets do not hoard gigabytes of artwork.
        let gib = 1024 * 1024 * 1024_u64;
        assert_eq!(cover_budget(16 * gib), 16 * gib / 100 * 4);
        assert_eq!(cover_budget(gib), COVER_BUDGET_FLOOR);
        assert_eq!(cover_budget(0), COVER_BUDGET_FLOOR);
        assert_eq!(cover_budget(100 * gib), COVER_BUDGET_CEILING);
    }

    #[test]
    fn the_budget_splits_between_originals_and_the_pixel_slice() {
        let directory = tempdir().unwrap();
        let budget = 256 * 1024 * 1024_u64;
        let cache = CoverCache::new(directory.path(), budget).unwrap();
        assert_eq!(
            cache.original_limit.load(Ordering::Relaxed),
            budget - PIXEL_LIMIT_BYTES
        );
        assert_eq!(cache.pixel_limit, PIXEL_LIMIT_BYTES);
    }
}
