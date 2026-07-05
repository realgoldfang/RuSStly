#![allow(dead_code)]

use std::fs::File;
use std::io::BufReader;
use std::sync::Mutex;
use std::time::Duration;

use rodio::{Decoder, OutputStream, Sink, Source};

pub struct Player {
    _stream: OutputStream,
    sink: Sink,
    current_episode_id: Mutex<Option<i64>>,
    current_path: Mutex<Option<String>>,
    seek_offset: Mutex<Duration>,
    total_duration: Mutex<Option<Duration>>,
    volume: Mutex<f32>,
    speed: Mutex<f32>,
}

impl Player {
    pub fn new() -> Self {
        log::info!("Initializing audio output stream");
        let result = OutputStream::try_default();
        let (_stream, stream_handle) = match result {
            Ok(s) => s,
            Err(e) => {
                log::error!("Failed to open audio output: {}", e);
                panic!("Failed to open audio output: {}", e);
            }
        };
        log::info!("Audio output stream opened successfully");
        let sink = Sink::try_new(&stream_handle).expect("Failed to create audio sink");
        Self {
            _stream,
            sink,
            current_episode_id: Mutex::new(None),
            current_path: Mutex::new(None),
            seek_offset: Mutex::new(Duration::ZERO),
            total_duration: Mutex::new(None),
            volume: Mutex::new(1.0),
            speed: Mutex::new(1.0),
        }
    }

    fn create_source(&self, path: &str, seek_pos: Option<Duration>) -> Option<Box<dyn Source<Item = f32> + Send>> {
        use rodio::source::Source as _;

        log::debug!("Creating audio source: path={}, seek_pos={:?}", path, seek_pos);
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                log::error!("Failed to open audio file {}: {}", path, e);
                return None;
            }
        };
        let mut decoder = match Decoder::new(BufReader::new(file)) {
            Ok(d) => d,
            Err(e) => {
                log::error!("Failed to decode audio file {}: {}", path, e);
                return None;
            }
        };
        log::debug!("Audio file decoded successfully: path={}", path);
        let total = decoder.total_duration();
        *self.total_duration.lock().unwrap() = total;

        if let Some(pos) = seek_pos {
            let clamped = if let Some(t) = total {
                pos.min(t)
            } else {
                pos
            };
            let _ = decoder.try_seek(clamped);
            *self.seek_offset.lock().unwrap() = clamped;
        } else {
            *self.seek_offset.lock().unwrap() = Duration::ZERO;
        }

        let s = *self.speed.lock().unwrap();
        let converted = decoder.convert_samples::<f32>();
        if (s - 1.0).abs() > 0.01 {
            Some(Box::new(converted.speed(s)))
        } else {
            Some(Box::new(converted))
        }
    }

    pub fn load_and_play(&self, episode_id: i64, path: &str, seek_to: f64) {
        let seek_pos = if seek_to > 0.0 {
            Some(Duration::from_secs_f64(seek_to))
        } else {
            None
        };

        let source = match self.create_source(path, seek_pos) {
            Some(s) => s,
            None => {
                eprintln!("Failed to load audio file: {}", path);
                return;
            }
        };

        self.sink.stop();
        self.sink.append(source);
        *self.current_episode_id.lock().unwrap() = Some(episode_id);
        *self.current_path.lock().unwrap() = Some(path.to_string());

        let vol = *self.volume.lock().unwrap();
        self.sink.set_volume(vol);
        self.sink.play();
    }

    pub fn play(&self) {
        self.sink.play();
    }

    pub fn pause(&self) {
        self.sink.pause();
    }

    pub fn toggle_play_pause(&self) {
        if self.sink.is_paused() {
            self.sink.play();
        } else {
            self.sink.pause();
        }
    }

    pub fn stop(&self) {
        self.sink.stop();
        *self.current_episode_id.lock().unwrap() = None;
        *self.current_path.lock().unwrap() = None;
        *self.seek_offset.lock().unwrap() = Duration::ZERO;
    }

    pub fn current_position(&self) -> Duration {
        *self.seek_offset.lock().unwrap() + self.sink.get_pos()
    }

    pub fn seek_to(&self, position: Duration) {
        let path = self.current_path.lock().unwrap().clone();
        let ep_id = *self.current_episode_id.lock().unwrap();
        if let (Some(path), Some(ep_id)) = (path, ep_id) {
            if let Some(source) = self.create_source(&path, Some(position)) {
                self.sink.stop();
                self.sink.append(source);
                *self.current_episode_id.lock().unwrap() = Some(ep_id);
                *self.current_path.lock().unwrap() = Some(path);
                let vol = *self.volume.lock().unwrap();
                self.sink.set_volume(vol);
                self.sink.play();
            }
        }
    }

    pub fn skip_forward(&self, secs: f64) {
        let pos = self.current_position();
        let new_pos = pos + Duration::from_secs_f64(secs);
        if let Some(total) = self.total_duration() {
            self.seek_to(new_pos.min(total));
        } else {
            self.seek_to(new_pos);
        }
    }

    pub fn skip_backward(&self, secs: f64) {
        let pos = self.current_position();
        let new_pos = if pos > Duration::from_secs_f64(secs) {
            pos - Duration::from_secs_f64(secs)
        } else {
            Duration::ZERO
        };
        self.seek_to(new_pos);
    }

    pub fn set_volume(&self, vol: f32) {
        let clamped = vol.clamp(0.0, 1.0);
        *self.volume.lock().unwrap() = clamped;
        self.sink.set_volume(clamped);
    }

    pub fn get_volume(&self) -> f32 {
        *self.volume.lock().unwrap()
    }

    pub fn set_speed(&self, speed: f32) {
        let clamped = speed.clamp(0.5, 3.0);
        *self.speed.lock().unwrap() = clamped;
        if self.current_episode_id().is_some() {
            let pos = self.current_position();
            self.seek_to(pos);
        }
    }

    pub fn get_speed(&self) -> f32 {
        *self.speed.lock().unwrap()
    }

    pub fn is_playing(&self) -> bool {
        !self.sink.is_paused() && !self.sink.empty()
    }

    pub fn is_paused(&self) -> bool {
        self.sink.is_paused() && !self.sink.empty()
    }

    pub fn is_empty(&self) -> bool {
        self.sink.empty()
    }

    pub fn current_episode_id(&self) -> Option<i64> {
        *self.current_episode_id.lock().unwrap()
    }

    pub fn total_duration(&self) -> Option<Duration> {
        *self.total_duration.lock().unwrap()
    }
}
