// Symphonia
// Copyright (c) 2019-2022 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![warn(rust_2018_idioms)]
#![forbid(unsafe_code)]
// The following lints are allowed in all Symphonia crates. Please see clippy.toml for their
// justification.
#![allow(clippy::comparison_chain)]
#![allow(clippy::excessive_precision)]
#![allow(clippy::identity_op)]
#![allow(clippy::manual_range_contains)]

use codec_ms::AdpcmMsParameters;
use symphonia_core::support_codec;

use symphonia_core::audio::{AsAudioBufferRef, AudioBuffer, AudioBufferRef, Signal, SignalSpec};
use symphonia_core::codecs::{CodecDescriptor, CodecParameters, CodecType};
use symphonia_core::codecs::{Decoder, DecoderOptions, FinalizeResult};
use symphonia_core::codecs::{CODEC_TYPE_ADPCM_IMA_WAV, CODEC_TYPE_ADPCM_MS};
use symphonia_core::errors::{unsupported_error, Result};
use symphonia_core::formats::Packet;
use symphonia_core::io::ReadBytes;

mod codec_ima;
mod codec_ms;
mod common;

fn is_supported_adpcm_codec(codec_type: CodecType) -> bool {
    matches!(codec_type, CODEC_TYPE_ADPCM_MS | CODEC_TYPE_ADPCM_IMA_WAV)
}

enum ExtraCodecParameters {
    AdpcmMs(AdpcmMsParameters),
    AdpcIma,
}

impl ExtraCodecParameters {
    fn decode_mono<B: ReadBytes>(
        &self,
        stream: &mut B,
        buffer: &mut [i32],
        frames_per_block: usize,
    ) -> Result<()> {
        match *self {
            ExtraCodecParameters::AdpcmMs(ref ms_params) => {
                codec_ms::decode_mono(stream, buffer, frames_per_block, ms_params)
            }
            ExtraCodecParameters::AdpcIma => {
                codec_ima::decode_mono(stream, buffer, frames_per_block)
            }
        }
    }

    fn decode_stereo<B: ReadBytes>(
        &self,
        stream: &mut B,
        buffers: [&mut [i32]; 2],
        frames_per_block: usize,
    ) -> Result<()> {
        match *self {
            ExtraCodecParameters::AdpcmMs(ref ms_params) => {
                codec_ms::decode_stereo(stream, buffers, frames_per_block, ms_params)
            }
            ExtraCodecParameters::AdpcIma => {
                codec_ima::decode_stereo(stream, buffers, frames_per_block)
            }
        }
    }
}

/// Adaptive Differential Pulse Code Modulation (ADPCM) decoder.
pub struct AdpcmDecoder {
    params: CodecParameters,
    extra_params: ExtraCodecParameters,
    buf: AudioBuffer<i32>,
}

impl AdpcmDecoder {
    fn decode_inner(&mut self, packet: &Packet) -> Result<()> {
        let mut stream = packet.as_buf_reader();

        let frames_per_block = self.params.frames_per_block.unwrap() as usize;

        let block_count = packet.block_dur() as usize / frames_per_block;

        self.buf.clear();
        self.buf.render_reserved(Some(block_count * frames_per_block));

        let channel_count = self.buf.spec().channels.count();
        match channel_count {
            1 => {
                let buffer = self.buf.chan_mut(0);
                for block_id in 0..block_count {
                    let offset = frames_per_block * block_id;
                    let buffer_range = offset..(offset + frames_per_block);
                    let buffer = &mut buffer[buffer_range];
                    self.extra_params.decode_mono(&mut stream, buffer, frames_per_block)?;
                }
            }
            2 => {
                let buffers = self.buf.chan_pair_mut(0, 1);
                for block_id in 0..block_count {
                    let offset = frames_per_block * block_id;
                    let buffer_range = offset..(offset + frames_per_block);
                    let buffers =
                        [&mut buffers.0[buffer_range.clone()], &mut buffers.1[buffer_range]];
                    self.extra_params.decode_stereo(&mut stream, buffers, frames_per_block)?;
                }
            }
            _ => unreachable!(),
        }

        Ok(())
    }
}

impl Decoder for AdpcmDecoder {
    fn try_new(params: &CodecParameters, _options: &DecoderOptions) -> Result<Self> {
        // This decoder only supports certain ADPCM codecs.
        if !is_supported_adpcm_codec(params.codec) {
            return unsupported_error("adpcm: invalid codec type");
        }

        let frames = match params.max_frames_per_packet {
            Some(frames) => frames,
            _ => return unsupported_error("adpcm: maximum frames per packet is required"),
        };

        if params.frames_per_block.is_none() {
            return unsupported_error("adpcm: frames per block is required");
        }

        let rate = match params.sample_rate {
            Some(rate) => rate,
            _ => return unsupported_error("adpcm: sample rate is required"),
        };

        let spec = if let Some(channels) = params.channels {
            SignalSpec::new(rate, channels)
        } else if let Some(layout) = params.channel_layout {
            SignalSpec::new_with_layout(rate, layout)
        } else {
            return unsupported_error("adpcm: channels or channel_layout is required");
        };

        let extra_params = match params.codec {
            CODEC_TYPE_ADPCM_MS => {
                let extra_params = AdpcmMsParameters::from_extra_data(&params.extra_data)?;
                ExtraCodecParameters::AdpcmMs(extra_params)
            }
            CODEC_TYPE_ADPCM_IMA_WAV => ExtraCodecParameters::AdpcIma,
            _ => unreachable!(),
        };

        Ok(AdpcmDecoder {
            params: params.clone(),
            extra_params,
            buf: AudioBuffer::new(frames, spec),
        })
    }

    fn supported_codecs() -> &'static [CodecDescriptor] {
        &[
            support_codec!(CODEC_TYPE_ADPCM_MS, "adpcm_ms", "Microsoft ADPCM"),
            support_codec!(CODEC_TYPE_ADPCM_IMA_WAV, "adpcm_ima_wav", "ADPCM IMA WAV"),
        ]
    }

    fn reset(&mut self) {
        // No state is stored between packets, therefore do nothing.
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> Result<AudioBufferRef<'_>> {
        if let Err(e) = self.decode_inner(packet) {
            self.buf.clear();
            Err(e)
        } else {
            Ok(self.buf.as_audio_buffer_ref())
        }
    }

    fn finalize(&mut self) -> FinalizeResult {
        Default::default()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.buf.as_audio_buffer_ref()
    }
}
