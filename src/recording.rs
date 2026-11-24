use std::{
    collections::VecDeque,
    fs,
    io::{self, BufWriter},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use rvoip_audio_device::{AudioDirection, FrameSink};

const RATE: u32 = 8_000;
const ALIGNMENT_WINDOW: usize = RATE as usize / 5;

struct Frame {
    channel: usize,
    position: usize,
    samples: Vec<i16>,
}

pub struct Recording {
    sender: Arc<Mutex<Option<SyncSender<Frame>>>>,
    failed: Arc<AtomicBool>,
    writer: Option<JoinHandle<Result<Option<String>, hound::Error>>>,
}

impl Recording {
    pub fn start(directory: &Path) -> Result<(Self, FrameSink), String> {
        fs::create_dir_all(directory).map_err(|_| "Could not create the recordings folder.")?;
        let prefix = chrono::Local::now()
            .format("call-%Y%m%d-%H%M%S-")
            .to_string();
        let file = tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(".part")
            .tempfile_in(directory)
            .map_err(|_| "Could not create a call recording.")?;
        let destination = file.path().with_extension("wav");
        let (send, receive) = mpsc::sync_channel(100);
        let sender = Arc::new(Mutex::new(Some(send)));
        let failed = Arc::new(AtomicBool::new(false));
        let writer_failed = failed.clone();
        let writer = thread::Builder::new()
            .name("softphone-recording".into())
            .spawn(move || {
                let result = write_recording(file, destination, receive, &writer_failed);
                if result.is_err() {
                    writer_failed.store(true, Ordering::Relaxed);
                }
                result
            })
            .map_err(|_| "Could not start the call recording.")?;

        let capture = sender.clone();
        let capture_failed = failed.clone();
        let started = Instant::now();
        let sink: FrameSink = Arc::new(move |direction, frame| {
            let sender = capture.lock().unwrap();
            if let Some(sender) = sender.as_ref() {
                if capture_failed.load(Ordering::Relaxed) {
                    return;
                }
                // The app negotiates G.711, and the bridge sends 8 kHz mono PCM.
                if frame.sample_rate != RATE || frame.channels != 1 {
                    capture_failed.store(true, Ordering::Relaxed);
                    return;
                }
                let position =
                    (started.elapsed().as_micros() * u128::from(RATE) / 1_000_000) as usize;
                if sender
                    .try_send(Frame {
                        channel: usize::from(direction == AudioDirection::Output),
                        position,
                        samples: frame.samples.clone(),
                    })
                    .is_err()
                {
                    capture_failed.store(true, Ordering::Relaxed);
                }
            }
        });
        Ok((
            Self {
                sender,
                failed,
                writer: Some(writer),
            },
            sink,
        ))
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn finish(mut self) -> Result<Option<String>, String> {
        self.sender.lock().unwrap().take();
        self.writer
            .take()
            .unwrap()
            .join()
            .map_err(|_| "Could not save the call recording.")?
            .map_err(|_| "Could not save the call recording.".into())
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.sender.lock().unwrap().take();
    }
}

fn write_recording(
    mut file: tempfile::NamedTempFile,
    destination: PathBuf,
    receive: Receiver<Frame>,
    failed: &AtomicBool,
) -> Result<Option<String>, hound::Error> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = hound::WavWriter::new(BufWriter::new(file.as_file_mut()), spec)?;
    let mut pending = VecDeque::new();
    let mut written = 0;
    let mut next = [None::<usize>; 2];

    while let Ok(frame) = receive.recv() {
        // Keep each side continuous through small scheduling and packet delays.
        // Use elapsed time to preserve silence across longer gaps.
        let start = next[frame.channel]
            .filter(|next| next.abs_diff(frame.position) <= ALIGNMENT_WINDOW / 2)
            .unwrap_or(frame.position)
            .max(written);
        let end = start + frame.samples.len();
        next[frame.channel] = Some(end);
        let flush_to = end.saturating_sub(ALIGNMENT_WINDOW).max(written);
        while written < flush_to {
            let [local, remote] = pending.pop_front().unwrap_or_default();
            wav.write_sample(local)?;
            wav.write_sample(remote)?;
            written += 1;
        }
        pending.resize(pending.len().max(end - written), [0i16; 2]);
        let skip = written.saturating_sub(start);
        for (offset, sample) in frame.samples.into_iter().enumerate().skip(skip) {
            pending[start + offset - written][frame.channel] = sample;
        }
    }

    if failed.load(Ordering::Relaxed) {
        return Err(io::Error::other("call recording lost audio frames").into());
    }
    if next == [None; 2] {
        return Ok(None);
    }
    for [local, remote] in pending {
        wav.write_sample(local)?;
        wav.write_sample(remote)?;
    }
    wav.finalize()?;
    file.persist_noclobber(&destination)
        .map_err(|error| error.error)?;
    Ok(destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned()))
}

/// Resolves a stored recording name to a file inside `directory`, rejecting
/// anything that is not a plain `.wav` filename.
pub fn path(directory: &Path, filename: &str) -> Result<PathBuf, String> {
    let mut components = Path::new(filename).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || !filename.ends_with(".wav")
    {
        return Err("The recording filename is invalid.".into());
    }
    let path = directory.join(filename);
    if !path.is_file() {
        return Err("The recording file is unavailable.".into());
    }
    Ok(path)
}

/// Hands the recording to another app: the Android share sheet, or the file
/// manager showing the file on desktop.
pub fn share(directory: &Path, filename: &str) -> Result<(), String> {
    share_file(&path(directory, filename)?)
        .map_err(|_| "Could not share the call recording.".into())
}

/// Deletes the recording file. A file that is already gone counts as deleted.
pub fn delete(directory: &Path, filename: &str) -> Result<(), String> {
    match path(directory, filename) {
        Ok(path) => {
            fs::remove_file(path).map_err(|_| "Could not delete the call recording.".into())
        }
        Err(error) if error == "The recording file is unavailable." => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(target_os = "android"))]
fn share_file(path: &Path) -> Result<(), ()> {
    // Desktop sharing reveals the file so it can be dragged into mail or chat.
    let mut command;
    #[cfg(target_os = "macos")]
    {
        command = std::process::Command::new("open");
        command.arg("-R").arg(path);
    }
    #[cfg(target_os = "windows")]
    {
        command = std::process::Command::new("explorer");
        command.arg(format!("/select,{}", path.display()));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        command = std::process::Command::new("xdg-open");
        command.arg(path.parent().ok_or(())?);
    }
    let mut child = command.spawn().map_err(|_| ())?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "android")]
fn share_file(path: &Path) -> Result<(), ()> {
    use jni::{JavaVM, objects::JObject};

    let context = ndk_context::android_context();
    // android-activity owns the VM and global Application reference.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(|_| ())?;
    let mut env = vm.attach_current_thread().map_err(|_| ())?;
    let application = unsafe { JObject::from_raw(context.context().cast()) };
    let path = env.new_string(path.to_str().ok_or(())?).map_err(|_| ())?;
    let result = env.call_method(
        &application,
        "shareRecording",
        "(Ljava/lang/String;)V",
        &[(&path).into()],
    );
    if result.is_err() {
        let _ = env.exception_clear();
    }
    result.map(|_| ()).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvoip_sip::AudioFrame;

    #[test]
    fn recording_writes_both_channels_to_a_finalized_wav() {
        let directory = tempfile::tempdir().unwrap();
        let (recording, capture) = Recording::start(directory.path()).unwrap();
        capture(
            AudioDirection::Input,
            &AudioFrame::new(vec![123; 160], RATE, 1, 0),
        );
        capture(
            AudioDirection::Output,
            &AudioFrame::new(vec![-456; 160], RATE, 1, 0),
        );
        let filename = recording.finish().unwrap().unwrap();
        assert!(filename.ends_with(".wav"));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        let mut reader = hound::WavReader::open(directory.path().join(filename)).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, RATE);
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            samples
                .chunks_exact(2)
                .filter(|pair| pair[0] == 123)
                .count(),
            160
        );
        assert_eq!(
            samples
                .chunks_exact(2)
                .filter(|pair| pair[1] == -456)
                .count(),
            160
        );
    }

    #[test]
    fn recording_without_audio_leaves_no_file() {
        let directory = tempfile::tempdir().unwrap();
        let (recording, _capture) = Recording::start(directory.path()).unwrap();
        assert_eq!(recording.finish().unwrap(), None);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn recording_names_are_confined_to_the_recordings_folder() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("call.wav"), b"x").unwrap();
        assert!(path(directory.path(), "call.wav").is_ok());
        for name in ["../call.wav", "/etc/passwd.wav", "a/b.wav", "call.txt", ""] {
            assert!(path(directory.path(), name).is_err(), "{name}");
        }
    }

    #[test]
    fn deleting_removes_the_file_and_tolerates_a_missing_one() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("call.wav"), b"x").unwrap();
        delete(directory.path(), "call.wav").unwrap();
        assert!(!directory.path().join("call.wav").exists());
        delete(directory.path(), "call.wav").unwrap();
        assert!(delete(directory.path(), "../call.wav").is_err());
    }
}
