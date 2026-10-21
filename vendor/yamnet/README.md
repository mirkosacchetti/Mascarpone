# YAMNet, in ONNX

Google's YAMNet, the AudioSet audio event classifier (521 classes, a
1024-wide embedding), as exported to ONNX with tf2onnx by Audio Magic:
https://huggingface.co/audiomagic/yamnet-onnx — a straight conversion of
`google/yamnet` version 1 (formerly `tfhub.dev/google/yamnet/1`), no
weights changed. Apache 2.0, the licence beside. Input: a mono waveform
at 16 kHz; outputs: the scores by frame of 0.96 s every 0.48 s, the
embeddings by frame, the log-mel spectrogram.

Mascarpone runs it on every file of the library and feeds its
embeddings to Essentia's voice/instrumental head (`vendor/essentia-models`).
