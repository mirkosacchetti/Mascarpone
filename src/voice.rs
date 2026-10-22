use anyhow::{anyhow, Result};
use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;

const RATE: usize = 16000;
const WINDOW: usize = 10 * RATE;
const FRAME: usize = 15600;
const HOP: usize = 7680;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voice {
    pub mean: f32,
    pub share: f32,
}

impl Voice {
    pub fn vocal(&self) -> bool {
        self.mean > 0.5
    }
}

/// YAMNet and the voice head on it, run by onnxruntime on one thread:
/// the archive runs a track per thread, so each thread loads its own.
pub struct Models {
    _environment: Environment,
    yamnet: Session,
    head: Session,
}

impl Models {
    pub fn load() -> Result<Models> {
        let environment = ort::init().with_name("mascarpone").build().map_err(err)?;
        let session = |bytes: &[u8]| -> Result<Session> {
            let builder = Session::builder(&environment).map_err(err)?;
            let builder = builder.with_optimization_level(GraphOptimizationLevel::Level3).map_err(|e| anyhow!("onnxruntime: {e}"))?;
            let mut builder = builder.with_intra_threads(1).map_err(|e| anyhow!("onnxruntime: {e}"))?;
            builder.commit_from_memory(bytes).map_err(err)
        };
        let yamnet = session(include_bytes!("../vendor/yamnet/yamnet.onnx"))?;
        let head = session(include_bytes!("../vendor/essentia-models/voice_instrumental-audioset-yamnet-1.onnx"))?;
        Ok(Models { _environment: environment, yamnet, head })
    }

    pub fn hear(&mut self, samples: &[f32], rate: usize) -> Result<Voice> {
        let sound = resample(samples, rate);
        let mut sum = 0.0;
        let mut voiced = 0;
        let mut frames = 0;
        let mut start = 0;
        while start < sound.len() {
            let end = (start + WINDOW).min(sound.len());
            let valid = end - start;
            if valid < FRAME {
                break;
            }
            let mut window = vec![0f32; WINDOW];
            window[..valid].copy_from_slice(&sound[start..end]);
            let input = Tensor::from_array(([WINDOW], window)).map_err(err)?;
            let out = self.yamnet.run(ort::inputs![input]).map_err(err)?;
            if out.len() < 2 {
                return Err(anyhow!("yamnet: no embeddings"));
            }
            let embeddings = &out[1];
            let probs = self.head.run(ort::inputs![embeddings]).map_err(err)?;
            let (shape, probs) = probs[0].try_extract_tensor::<f32>().map_err(err)?;
            let usable = ((valid - FRAME) / HOP + 1).min(shape[0] as usize);
            for f in 0..usable {
                let p = probs[f * 2 + 1];
                sum += p;
                if p > 0.5 {
                    voiced += 1;
                }
                frames += 1;
            }
            start += WINDOW;
        }
        if frames == 0 {
            return Err(anyhow!("too short to hear"));
        }
        Ok(Voice { mean: sum / frames as f32, share: voiced as f32 / frames as f32 })
    }
}

/// onnxruntime's errors are not Send, as anyhow wants: their text is.
fn err(e: ort::Error) -> anyhow::Error {
    anyhow!("onnxruntime: {e}")
}

fn resample(samples: &[f32], rate: usize) -> Vec<f32> {
    if rate == RATE {
        return samples.to_vec();
    }
    let ratio = rate as f64 / RATE as f64;
    let n = (samples.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let x = i as f64 * ratio;
        let j = x as usize;
        let t = (x - j as f64) as f32;
        let a = samples[j];
        let b = samples.get(j + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * t);
    }
    out
}

#[cfg(test)]
mod probe {
    /// `MASCARPONE_TRACK=<path> cargo test --release voice::probe -- --ignored --nocapture`
    /// times the decode, the voice and bliss on one track.
    #[test]
    #[ignore]
    fn timing() {
        use bliss_audio::decoder::{symphonia::SymphoniaDecoder, Decoder};
        use std::time::Instant;
        let path = std::env::var("MASCARPONE_TRACK").expect("MASCARPONE_TRACK");
        let t = Instant::now();
        let pre = <SymphoniaDecoder as Decoder>::decode(std::path::Path::new(&path)).unwrap();
        let seconds = pre.sample_array.len() as f32 / 22050.0;
        println!("decode {:.2}s for {seconds:.0}s of audio", t.elapsed().as_secs_f32());
        let t = Instant::now();
        let mut models = super::Models::load().unwrap();
        println!("models {:.2}s", t.elapsed().as_secs_f32());
        let t = Instant::now();
        let v = models.hear(&pre.sample_array, 22050).unwrap();
        println!("voice {:.2}s ({v:?})", t.elapsed().as_secs_f32());
        let t = Instant::now();
        let _ = bliss_audio::Song::analyze_series(&pre.sample_array).unwrap();
        println!("bliss {:.2}s", t.elapsed().as_secs_f32());
    }
}
