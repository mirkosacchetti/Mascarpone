use anyhow::{anyhow, Result};
use tract_onnx::prelude::*;

const RATE: usize = 16000;
const WINDOW: usize = 10 * RATE;
const FRAME: usize = 15600;
const HOP: usize = 7680;
const PER_WINDOW: usize = 20;
const EMBEDDING: usize = 1024;

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

pub struct Models {
    yamnet: TypedRunnableModel<TypedModel>,
    head: TypedRunnableModel<TypedModel>,
}

impl Models {
    pub fn load() -> Result<Models> {
        let yamnet = tract_onnx::onnx()
            .model_for_read(&mut &include_bytes!("../vendor/yamnet/yamnet.onnx")[..])?
            .with_input_fact(0, InferenceFact::dt_shape(f32::datum_type(), tvec!(WINDOW)))?
            .into_optimized()?
            .into_runnable()?;
        let head = tract_onnx::onnx()
            .model_for_read(&mut &include_bytes!("../vendor/essentia-models/voice_instrumental-audioset-yamnet-1.onnx")[..])?
            .with_input_fact(0, InferenceFact::dt_shape(f32::datum_type(), tvec!(PER_WINDOW, EMBEDDING)))?
            .into_optimized()?
            .into_runnable()?;
        Ok(Models { yamnet, head })
    }

    pub fn hear(&self, samples: &[f32], rate: usize) -> Result<Voice> {
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
            let input: Tensor = tract_ndarray::Array1::from(window).into();
            let out = self.yamnet.run(tvec!(input.into()))?;
            let embeddings = out.get(1).ok_or_else(|| anyhow!("yamnet: no embeddings"))?;
            let probs = self.head.run(tvec!(embeddings.clone()))?;
            let probs = probs[0].to_array_view::<f32>()?;
            let usable = ((valid - FRAME) / HOP + 1).min(probs.shape()[0]);
            for f in 0..usable {
                let p = probs[[f, 1]];
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
