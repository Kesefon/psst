use crate::{
    audio_decode::VorbisDecoder,
    audio_decrypt::AudioDecrypt,
    audio_key::AudioKey,
    cache::CacheHandle,
    cdn::{CdnHandle, CdnUrl},
    error::Error,
    item_id::{FileId, ItemId},
    protocol::metadata::mod_AudioFile::Format,
    stream_storage::{StreamReader, StreamStorage, StreamWriter},
    util::OffsetFile,
};
use std::{
    io,
    io::{BufReader, Seek, SeekFrom},
    path::PathBuf,
    sync::Arc,
    thread,
    thread::JoinHandle,
    time::Duration,
};

pub type FileAudioSource = VorbisDecoder<OffsetFile<AudioDecrypt<BufReader<StreamReader>>>>;

#[derive(Debug, Clone, Copy)]
pub struct AudioPath {
    pub item_id: ItemId,
    pub file_id: FileId,
    pub file_format: Format,
    pub duration: Duration,
}

pub enum AudioFile {
    Streamed {
        streamed_file: Arc<StreamedFile>,
        servicing_handle: JoinHandle<()>,
    },
    Cached {
        cached_file: CachedFile,
    },
}

impl AudioFile {
    pub fn compatible_audio_formats(preferred_bitrate: usize) -> &'static [Format] {
        match preferred_bitrate {
            96 => &[
                Format::OGG_VORBIS_96,
                Format::OGG_VORBIS_160,
                Format::OGG_VORBIS_320,
            ],
            160 => &[
                Format::OGG_VORBIS_160,
                Format::OGG_VORBIS_320,
                Format::OGG_VORBIS_96,
            ],
            320 => &[
                Format::OGG_VORBIS_320,
                Format::OGG_VORBIS_160,
                Format::OGG_VORBIS_96,
            ],
            _ => unreachable!(),
        }
    }

    pub fn open(path: AudioPath, cdn: CdnHandle, cache: CacheHandle) -> Result<Self, Error> {
        let cached_file = cache.audio_file_path(path.file_id);
        if cached_file.exists() {
            let cached_file = CachedFile::open(path, cached_file)?;
            Ok(Self::Cached { cached_file })
        } else {
            let streamed_file = Arc::new(StreamedFile::open(path, cdn, cache)?);
            let servicing_handle = thread::spawn({
                let streamed_file = Arc::clone(&streamed_file);
                move || {
                    streamed_file
                        .service_streaming()
                        .expect("Streaming thread failed");
                }
            });
            Ok(Self::Streamed {
                streamed_file,
                servicing_handle,
            })
        }
    }

    pub fn path(&self) -> AudioPath {
        match self {
            Self::Streamed { streamed_file, .. } => streamed_file.path,
            Self::Cached { cached_file, .. } => cached_file.path,
        }
    }

    pub fn audio_source(&self, key: AudioKey) -> Result<FileAudioSource, Error> {
        let reader = match self {
            Self::Streamed { streamed_file, .. } => streamed_file.storage.reader()?,
            Self::Cached { cached_file, .. } => cached_file.storage.reader()?,
        };
        let reader = BufReader::new(reader);
        let reader = AudioDecrypt::new(key, reader);
        let reader = OffsetFile::new(reader, self.header_length())?;
        let reader = VorbisDecoder::new(reader)?;
        Ok(reader)
    }

    fn header_length(&self) -> u64 {
        match self.path().file_format {
            Format::OGG_VORBIS_96 | Format::OGG_VORBIS_160 | Format::OGG_VORBIS_320 => 167,
            _ => 0,
        }
    }
}

pub struct StreamedFile {
    path: AudioPath,
    storage: StreamStorage,
    url: CdnUrl,
    cdn: CdnHandle,
    cache: CacheHandle,
}

impl StreamedFile {
    fn open(path: AudioPath, cdn: CdnHandle, cache: CacheHandle) -> Result<StreamedFile, Error> {
        // First, we need to resolve URL of the file contents.
        let url = cdn.resolve_audio_file_url(path.file_id)?;
        log::debug!("resolved file URL: {:?}", url.url);

        // How many bytes we request in the first chunk.  Lower amount means lower
        // initial latency, but should be high enough that the audio decoder can
        // initialize without further reads, otherwise `AudioFile::audio_source` will
        // get stuck, as the loading routine is not started yet.
        const INITIAL_REQUEST_LENGTH: u64 = 1024 * 6;

        // Send the initial request, that gives us the total file length and the
        // beginning of the contents.  The initial data should be big enough for the
        // audio decoder to bootstrap, without waiting.  Use the total length for
        // creating the backing data storage.
        let (total_length, mut initial_data) =
            cdn.fetch_file_range(&url.url, 0, INITIAL_REQUEST_LENGTH)?;
        let storage = StreamStorage::new(total_length)?;

        // Pipe the initial data from the request body into storage.
        io::copy(&mut initial_data, &mut storage.writer()?)?;

        Ok(StreamedFile {
            path,
            storage,
            url,
            cdn,
            cache,
        })
    }

    fn service_streaming(&self) -> Result<(), Error> {
        let mut last_url = self.url.clone();
        let mut fresh_url = || -> Result<CdnUrl, Error> {
            if last_url.is_expired() {
                last_url = self.cdn.resolve_audio_file_url(self.path.file_id)?;
            }
            Ok(last_url.clone())
        };

        while let Ok((position, length)) = self.storage.receiver().recv() {
            log::trace!("downloading {}..{}", position, position + length);

            let thread_name = format!(
                "cdn-{}-{}..{}",
                self.path.file_id.to_base16(),
                position,
                position + length
            );
            // TODO: We spawn threads here without any accounting.  Seems wrong.
            thread::Builder::new().name(thread_name).spawn({
                // TODO: Do not bury the whole servicing loop in case the URL renewal fails.
                let url = fresh_url()?.url.clone();
                let cdn = self.cdn.clone();
                let cache = self.cache.clone();
                let mut writer = self.storage.writer()?;
                let file_path = self.storage.path().to_path_buf();
                let file_id = self.path.file_id;
                move || {
                    match load_range(&mut writer, cdn, &url, position, length) {
                        Ok(_) => {
                            // If the file is completely downloaded, copy it to cache.
                            if writer.is_complete() && !cache.audio_file_path(file_id).exists() {
                                // TODO: We should do this atomically.
                                if let Err(err) = cache.save_audio_file(file_id, file_path) {
                                    log::warn!("failed to save audio file to cache: {:?}", err);
                                }
                            }
                        }
                        Err(err) => {
                            log::error!("failed to download: {}", err);
                            // Range failed to download, remove it from the requested set.
                            writer.mark_as_not_requested(position, length);
                        }
                    }
                }
            });
        }
        Ok(())
    }
}

pub struct CachedFile {
    path: AudioPath,
    storage: StreamStorage,
}

impl CachedFile {
    fn open(path: AudioPath, file_path: PathBuf) -> Result<Self, Error> {
        Ok(Self {
            path,
            storage: StreamStorage::from_complete_file(file_path)?,
        })
    }
}

fn load_range(
    writer: &mut StreamWriter,
    cdn: CdnHandle,
    url: &str,
    position: u64,
    length: u64,
) -> Result<(), Error> {
    // Download range of data from the CDN.  Block until we a have reader of the
    // request body.
    let (_total_length, mut reader) = cdn.fetch_file_range(url, position, length)?;

    // Pipe it into storage. Blocks until fully written, but readers sleeping on
    // this file should be notified as soon as their offset is covered.
    writer.seek(SeekFrom::Start(position))?;
    io::copy(&mut reader, writer)?;

    Ok(())
}
