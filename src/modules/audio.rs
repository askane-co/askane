use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::audio_backend::{
    AudioStateSnapshot, InitOpts, bus_exists, close_music_stream, control_music, current_sink_name,
    ensure_initialized_auto, get_bus_state, get_music_shared, get_or_create_bus, get_state,
    get_voice_shared, init_engine, is_initialized, play_music_stream, play_pcm_voice, poll_events,
    send_command, shutdown_engine, tick_virtual, update_deadline,
};
use crate::buffer::Elem;
use crate::media::audio::mixer::{MixerCommand, OverflowPolicy, render_sound_offline};
use crate::media::audio::sound::{AudioEvent, MusicStatus, SoundData};
use crate::media::audio::synth::{
    NoiseColor, NoiseOpts, ToneOpts, Waveform, from_samples_f32, synth_noise, synth_tone,
};
use crate::media::audio::wav::{decode_wav_sound, encode_wav_s16le};
use crate::media::audio::{
    ENGINE_RATE, MAX_BUSES, MAX_FADE_MS, MAX_MUSICS, MAX_VOICES, QUANTUM,
};
use crate::media::cache::read_asset_bytes;
use crate::media::err_bad_opt;
use crate::modules::{NativeFnPtr, build_module};
use crate::value::{FastMap, StructDef, StructInstance, Value, new_dict_with_capacity};
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "audio",
        &[
            ("init", audio_init),
            ("shutdown", audio_shutdown),
            ("sink_name", audio_sink_name),
            ("state", audio_state),
            ("poll_errors", audio_poll_errors),
            ("load", audio_load),
            ("stream", audio_stream),
            ("music_load", audio_stream),
            ("music_play", audio_music_play),
            ("music_stop", audio_music_stop),
            ("music_pause", audio_music_pause),
            ("music_resume", audio_music_resume),
            ("music_seek", audio_music_seek),
            ("music_position_ms", audio_music_position_ms),
            ("music_duration_ms", audio_music_duration_ms),
            ("music_set_volume", audio_music_set_volume),
            ("music_set_loop", audio_music_set_loop),
            ("music_set_loop_points", audio_music_set_loop_points),
            ("music_fade_to", audio_music_fade_to),
            ("music_state", audio_music_state),
            ("music_close", audio_music_close),
            ("from_samples", audio_from_samples),
            ("tone", audio_tone),
            ("noise", audio_noise),
            ("render_offline", audio_render_offline),
            ("bus", audio_bus),
            ("bus_new", audio_bus_new),
            ("bus_set_volume", audio_bus_set_volume),
            ("bus_set_mute", audio_bus_set_mute),
            ("bus_set_effect", audio_bus_set_effect),
            ("bus_stop_all", audio_bus_stop_all),
            ("play", audio_play),
            ("play_at", audio_play_at),
            ("stop", audio_stop),
            ("stop_all", audio_stop_all),
            ("pause_all", audio_pause_all),
            ("resume_all", audio_resume_all),
            ("set_listener", audio_set_listener),
            ("clock_ms", audio_clock_ms),
            ("tick", audio_tick),
            ("voice_stop", audio_voice_stop),
            ("voice_pause", audio_voice_pause),
            ("voice_resume", audio_voice_resume),
            ("voice_set_volume", audio_voice_set_volume),
            ("voice_set_pan", audio_voice_set_pan),
            ("voice_set_pitch", audio_voice_set_pitch),
            ("voice_set_loop", audio_voice_set_loop),
            ("voice_set_loop_points", audio_voice_set_loop_points),
            ("voice_fade_to", audio_voice_fade_to),
            ("voice_set_pos3", audio_voice_set_pos3),
            ("voice_is_playing", audio_voice_is_playing),
            ("voice_position_ms", audio_voice_position_ms),
        ],
        &[
            ("ENGINE_RATE", Value::Int(ENGINE_RATE as i64)),
            ("SAMPLE_RATE", Value::Int(ENGINE_RATE as i64)),
            ("CHANNELS", Value::Int(2)),
            ("QUANTUM", Value::Int(QUANTUM as i64)),
            ("MAX_VOICES", Value::Int(MAX_VOICES as i64)),
            ("MAX_BUSES", Value::Int(MAX_BUSES as i64)),
            ("MAX_MUSICS", Value::Int(MAX_MUSICS as i64)),
        ],
    )
}

fn make_handle_def(
    name: &str,
    fields: &[&str],
    methods: &[(&'static str, NativeFnPtr)],
) -> Rc<StructDef> {
    let field_names: Vec<String> = fields.iter().map(|s| (*s).to_string()).collect();
    let mut field_index = FastMap::default();
    field_index.reserve(field_names.len());
    let mut field_defaults = Vec::with_capacity(field_names.len());
    for (i, f) in field_names.iter().enumerate() {
        field_index.insert(f.clone(), i);
        field_defaults.push(None);
    }
    let mut method_map = new_dict_with_capacity(methods.len());
    for &(mname, func) in methods {
        method_map.insert(mname.to_string(), Value::native_fn(mname, func));
    }
    Rc::new(StructDef {
        ic_id: crate::bytecode::next_ic_id(),
        name: name.to_string(),
        field_names,
        field_index,
        field_defaults,
        methods: method_map,
    })
}

thread_local! {
    static VOICE_DEF: Rc<StructDef> = make_handle_def(
        "AudioVoice",
        &["id"],
        &[
            ("stop", audio_voice_stop),
            ("close", audio_voice_stop),
            ("pause", audio_voice_pause),
            ("resume", audio_voice_resume),
            ("set_volume", audio_voice_set_volume),
            ("set_pan", audio_voice_set_pan),
            ("set_pitch", audio_voice_set_pitch),
            ("set_loop", audio_voice_set_loop),
            ("set_loop_points", audio_voice_set_loop_points),
            ("fade_to", audio_voice_fade_to),
            ("set_pos3", audio_voice_set_pos3),
            ("is_playing", audio_voice_is_playing),
            ("position_ms", audio_voice_position_ms),
        ],
    );
    static MUSIC_DEF: Rc<StructDef> = make_handle_def(
        "AudioMusic",
        &["id"],
        &[
            ("play", audio_music_play),
            ("stop", audio_music_stop),
            ("pause", audio_music_pause),
            ("resume", audio_music_resume),
            ("seek", audio_music_seek),
            ("position_ms", audio_music_position_ms),
            ("duration_ms", audio_music_duration_ms),
            ("set_volume", audio_music_set_volume),
            ("set_loop", audio_music_set_loop),
            ("set_loop_points", audio_music_set_loop_points),
            ("fade_to", audio_music_fade_to),
            ("state", audio_music_state),
            ("close", audio_music_close),
        ],
    );
    static BUS_DEF: Rc<StructDef> = make_handle_def(
        "AudioBus",
        &["id"],
        &[
            ("name", audio_bus_name),
            ("volume", audio_bus_volume),
            ("set_volume", audio_bus_set_volume),
            ("muted", audio_bus_muted),
            ("set_mute", audio_bus_set_mute),
            ("set_effect", audio_bus_set_effect),
            ("stop_all", audio_bus_stop_all),
        ],
    );
}

fn make_voice_handle(vid: u64) -> Value {
    VOICE_DEF.with(|def| {
        Value::StructInstance(Rc::new(StructInstance {
            def: Rc::clone(def),
            fields: RefCell::new(vec![Value::Int(vid as i64)]),
        }))
    })
}

fn make_music_handle(mid: u64) -> Value {
    MUSIC_DEF.with(|def| {
        Value::StructInstance(Rc::new(StructInstance {
            def: Rc::clone(def),
            fields: RefCell::new(vec![Value::Int(mid as i64)]),
        }))
    })
}

fn make_bus_handle(bid: u16) -> Value {
    BUS_DEF.with(|def| {
        Value::StructInstance(Rc::new(StructInstance {
            def: Rc::clone(def),
            fields: RefCell::new(vec![Value::Int(bid as i64)]),
        }))
    })
}

fn extract_handle_id(val: &Value, expected_struct: &str, ctx: &str) -> Result<u64, String> {
    match val {
        Value::Int(i) if *i >= 0 => Ok(*i as u64),
        Value::StructInstance(inst) if inst.def.name == expected_struct => {
            match inst.fields.borrow().first() {
                Some(Value::Int(i)) if *i >= 0 => Ok(*i as u64),
                _ => Err(err_bad_opt(format!("{} invalid {} id", ctx, expected_struct))),
            }
        }
        other => Err(err_bad_opt(format!(
            "{} expects {} or non-negative int id, got '{}'",
            ctx,
            expected_struct,
            other.type_name()
        ))),
    }
}

fn num_as_f64(val: &Value, ctx: &str) -> Result<f64, String> {
    match val {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) if f.is_finite() => Ok(*f),
        other => Err(err_bad_opt(format!(
            "{} expects finite number, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

fn num_as_f32(val: &Value, ctx: &str) -> Result<f32, String> {
    Ok(num_as_f64(val, ctx)? as f32)
}

fn num_as_u32(val: &Value, ctx: &str) -> Result<u32, String> {
    match val {
        Value::Int(i) if *i >= 0 && *i <= (u32::MAX as i64) => Ok(*i as u32),
        Value::Float(f) if f.is_finite() && *f >= 0.0 && *f <= (u32::MAX as f64) => {
            Ok(*f as u32)
        }
        other => Err(err_bad_opt(format!(
            "{} expects non-negative u32, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

fn num_as_u64(val: &Value, ctx: &str) -> Result<u64, String> {
    match val {
        Value::Int(i) if *i >= 0 => Ok(*i as u64),
        other => Err(err_bad_opt(format!(
            "{} expects non-negative int, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

fn extract_vec3(val: &Value, ctx: &str) -> Result<[f32; 3], String> {
    match val {
        Value::Vec3(v) => Ok(**v),
        Value::List(l) => {
            let items = l.borrow();
            if items.len() != 3 {
                return Err(err_bad_opt(format!("{} expects 3-element [x, y, z] list", ctx)));
            }
            Ok([
                num_as_f32(&items[0], ctx)?,
                num_as_f32(&items[1], ctx)?,
                num_as_f32(&items[2], ctx)?,
            ])
        }
        other => Err(err_bad_opt(format!(
            "{} expects vec3 or [x, y, z], got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

pub(crate) fn parse_loop_override_opt(val: Option<&Value>) -> Result<Option<(u64, u64)>, String> {
    let Some(v) = val else {
        return Ok(None);
    };
    if matches!(v, Value::Unit) {
        return Ok(None);
    }
    let Value::Dict(dict) = v else {
        return Err(err_bad_opt(format!(
            "audio load opts must be dict, got '{}'",
            v.type_name()
        )));
    };
    let map = dict.borrow();
    if let Some(lp) = map.get("loop_points").or_else(|| map.get("loop")) {
        if let Value::List(list) = lp {
            let items = list.borrow();
            if items.len() != 2 {
                return Err(err_bad_opt("loop_points expects [start_frame, end_frame]"));
            }
            let a = num_as_u64(&items[0], "loop_points[0]")?;
            let b = num_as_u64(&items[1], "loop_points[1]")?;
            return Ok(Some((a, b)));
        } else if !matches!(lp, Value::Bool(_) | Value::Unit) {
            return Err(err_bad_opt("loop_points expects [start_frame, end_frame]"));
        }
    }
    let ls = map.get("loop_start");
    let le = map.get("loop_end");
    match (ls, le) {
        (Some(a), Some(b)) => Ok(Some((
            num_as_u64(a, "loop_start")?,
            num_as_u64(b, "loop_end")?,
        ))),
        (None, None) => Ok(None),
        _ => Err(err_bad_opt(
            "loop_start and loop_end must both be supplied together",
        )),
    }
}

fn state_snapshot_to_value(snap: AudioStateSnapshot) -> Value {
    let mut map = new_dict_with_capacity(14);
    map.insert("initialized".to_string(), Value::Bool(snap.initialized));
    map.insert("backend".to_string(), Value::from_str(snap.backend));
    map.insert("sample_rate".to_string(), Value::Int(snap.sample_rate as i64));
    map.insert("voices".to_string(), Value::Int(snap.voices as i64));
    map.insert("max_voices".to_string(), Value::Int(snap.max_voices as i64));
    map.insert("buses".to_string(), Value::Int(snap.buses as i64));
    map.insert("musics".to_string(), Value::Int(snap.musics as i64));
    map.insert(
        "frames_played".to_string(),
        Value::Int(snap.frames_played as i64),
    );
    map.insert("clock_ms".to_string(), Value::Int(snap.clock_ms as i64));
    map.insert("underruns".to_string(), Value::Int(snap.underruns as i64));
    map.insert(
        "dropped_events".to_string(),
        Value::Int(snap.dropped_events as i64),
    );
    map.insert(
        "pending_cmds".to_string(),
        Value::Int(snap.pending_cmds as i64),
    );
    map.insert(
        "leaked_threads".to_string(),
        Value::Int(snap.leaked_threads as i64),
    );
    map.insert("fatal".to_string(), Value::Bool(snap.fatal));
    Value::dict(map)
}

fn audio_init(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() > 1 {
        return Err(err_bad_opt("audio.init() expects at most 1 options dict"));
    }
    let mut opts = InitOpts::default();
    if let Some(arg0) = args.first() {
        if !matches!(arg0, Value::Unit) {
            let Value::Dict(dict) = arg0 else {
                return Err(err_bad_opt(format!(
                    "audio.init() opts must be dict, got '{}'",
                    arg0.type_name()
                )));
            };
            for (k, v) in dict.borrow().iter() {
                match k.as_str() {
                    "sink" | "backend" => {
                        let s = v.as_str().ok_or_else(|| {
                            err_bad_opt("audio.init() option 'sink' must be str")
                        })?;
                        opts.sink = Some(s.to_string());
                    }
                    "sample_rate" => {
                        opts.sample_rate = num_as_u32(v, "audio.init().sample_rate")?;
                    }
                    "max_voices" => {
                        let mv = num_as_u32(v, "audio.init().max_voices")?;
                        opts.max_voices = mv as usize;
                    }
                    "buffer_ms" => {
                        opts.buffer_ms = num_as_u32(v, "audio.init().buffer_ms")?.clamp(5, 1000);
                    }
                    other => {
                        return Err(err_bad_opt(format!(
                            "audio.init() unknown option '{}'",
                            other
                        )));
                    }
                }
            }
        }
    }
    let snap = init_engine(
        &opts,
        vm.is_deterministic(),
        Some(Arc::clone(vm.budget_core())),
        vm.remaining_timeout_ms(),
    )?;
    Ok(state_snapshot_to_value(snap))
}

fn audio_shutdown(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if !args.is_empty() {
        return Err(err_bad_opt("audio.shutdown() expects 0 arguments"));
    }
    shutdown_engine();
    Ok(Value::Unit)
}

fn audio_sink_name(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if !args.is_empty() {
        return Err(err_bad_opt("audio.sink_name() expects 0 arguments"));
    }
    Ok(Value::from_str(current_sink_name()))
}

fn audio_state(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if !args.is_empty() {
        return Err(err_bad_opt("audio.state() expects 0 arguments"));
    }
    update_deadline(vm.remaining_timeout_ms());
    Ok(state_snapshot_to_value(get_state()))
}

fn audio_poll_errors(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if !args.is_empty() {
        return Err(err_bad_opt("audio.poll_errors() expects 0 arguments"));
    }
    let events = poll_events();
    let mut list = Vec::with_capacity(events.len());
    for ev in events {
        let mut d = new_dict_with_capacity(4);
        match ev {
            AudioEvent::Fallback { from, to, reason } => {
                d.insert("kind".to_string(), Value::from_str("fallback"));
                d.insert("from".to_string(), Value::from_str(from));
                d.insert("to".to_string(), Value::from_str(to));
                d.insert("message".to_string(), Value::from_str(reason));
            }
            AudioEvent::Underrun { frames } => {
                d.insert("kind".to_string(), Value::from_str("underrun"));
                d.insert("frames".to_string(), Value::Int(frames as i64));
                d.insert(
                    "message".to_string(),
                    Value::from_str(format!("audio underrun ({} frames)", frames)),
                );
            }
            AudioEvent::StreamError { music_id, message } => {
                d.insert("kind".to_string(), Value::from_str("stream_error"));
                d.insert("music_id".to_string(), Value::Int(music_id as i64));
                d.insert("message".to_string(), Value::from_str(message));
            }
            AudioEvent::DeviceStopped { reason } => {
                d.insert("kind".to_string(), Value::from_str("device_stopped"));
                d.insert("message".to_string(), Value::from_str(reason));
            }
        }
        list.push(Value::dict(d));
    }
    Ok(Value::List(Rc::new(RefCell::new(list))))
}

fn audio_load(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("audio.load() expects (path_or_bytes, [opts])"));
    }
    let loop_override = parse_loop_override_opt(args.get(1))?;
    let bytes: Vec<u8> = match &args[0] {
        Value::Str(path) => read_asset_bytes(vm, path)?.0,
        other => other.as_bytes_cow("audio.load()")?.into_owned(),
    };
    let snd = decode_wav_sound(
        &bytes,
        loop_override,
        Some(vm.budget_core()),
        Some(&|| vm.check_timeout()),
    )?;
    Ok(Value::Sound(Rc::new(snd)))
}

fn audio_stream(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    vm.check_timeout()?;
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("audio.stream() expects (path_or_bytes, [opts])"));
    }
    let raw_bytes: Vec<u8> = match &args[0] {
        Value::Str(path) => read_asset_bytes(vm, path)?.0,
        other => other.as_bytes_cow("audio.stream()")?.into_owned(),
    };

    let mut chunk_delay_ms = 0u32;
    let mut opt_loop = None;
    let mut opt_loop_points = None;
    let mut opt_vol = None;

    if let Some(arg1) = args.get(1) {
        if !matches!(arg1, Value::Unit) {
            let Value::Dict(dict) = arg1 else {
                return Err(err_bad_opt("audio.stream() opts must be dict"));
            };
            let map = dict.borrow();
            if let Some(d) = map.get("chunk_delay_ms") {
                chunk_delay_ms = num_as_u32(d, "audio.stream().chunk_delay_ms")?.min(10_000);
            }
            if let Some(lp) = map.get("loop").or_else(|| map.get("looped")) {
                if let Value::Bool(b) = lp {
                    opt_loop = Some(*b);
                }
            }
            if let Some(v) = map.get("volume") {
                opt_vol = Some(num_as_f32(v, "audio.stream().volume")?.clamp(0.0, 4.0));
            }
            drop(map);
            opt_loop_points = parse_loop_override_opt(args.get(1))?;
        }
    }

    let (mid, _shared, _dur) = crate::audio_backend::create_music_stream(
        raw_bytes,
        chunk_delay_ms,
        Arc::clone(vm.budget_core()),
        vm.remaining_timeout_ms(),
    )?;

    if opt_loop.is_some() || opt_loop_points.is_some() || opt_vol.is_some() {
        control_music(mid, |slot, _cmds| {
            if let Some(lp) = opt_loop {
                slot.loop_mode = lp;
            }
            if let Some((a, b)) = opt_loop_points {
                slot.loop_points = Some((a, b));
            }
            if let Some(vol) = opt_vol {
                slot.volume = vol;
                slot.shared
                    .vol_milli
                    .store((vol * 1000.0) as u32, Ordering::Release);
            }
            Ok(())
        })?;
    }

    Ok(make_music_handle(mid))
}

fn audio_from_samples(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 3 {
        return Err(err_bad_opt(
            "audio.from_samples() expects (samples, [sample_rate_or_opts, channels])",
        ));
    }
    let mut sample_rate = ENGINE_RATE;
    let mut channels = 1u16;

    if let Some(arg1) = args.get(1) {
        if let Value::Dict(dict) = arg1 {
            if args.len() > 2 {
                return Err(err_bad_opt(
                    "audio.from_samples(samples, opts) expects at most 2 args",
                ));
            }
            for (k, v) in dict.borrow().iter() {
                match k.as_str() {
                    "sample_rate" | "rate" => {
                        sample_rate = num_as_u32(v, "from_samples().sample_rate")?;
                    }
                    "channels" | "ch" => {
                        let c = num_as_u32(v, "from_samples().channels")?;
                        channels = u16::try_from(c).map_err(|_| err_bad_opt("invalid channels"))?;
                    }
                    other => {
                        return Err(err_bad_opt(format!(
                            "audio.from_samples() unknown option '{}'",
                            other
                        )));
                    }
                }
            }
        } else if !matches!(arg1, Value::Unit) {
            sample_rate = num_as_u32(arg1, "audio.from_samples() sample_rate")?;
            if let Some(arg2) = args.get(2) {
                if !matches!(arg2, Value::Unit) {
                    let c = num_as_u32(arg2, "audio.from_samples() channels")?;
                    channels = u16::try_from(c).map_err(|_| err_bad_opt("invalid channels"))?;
                }
            }
        }
    }

    let samples: Vec<f32> = match &args[0] {
        Value::List(l) => {
            let items = l.borrow();
            let mut v = Vec::with_capacity(items.len());
            for item in items.iter() {
                v.push(num_as_f32(item, "from_samples() element")?);
            }
            v
        }
        Value::Buffer(b) => {
            let buf = b.borrow();
            let mut v = Vec::with_capacity(buf.len);
            match buf.elem {
                Elem::F32 => {
                    for chunk in buf.raw.chunks_exact(4).take(buf.len) {
                        v.push(f32::from_le_bytes(chunk.try_into().unwrap()));
                    }
                }
                Elem::I32 => {
                    for chunk in buf.raw.chunks_exact(4).take(buf.len) {
                        let s = i32::from_le_bytes(chunk.try_into().unwrap());
                        v.push(((s as f64) / 2147483648.0) as f32);
                    }
                }
                Elem::U32 => {
                    for chunk in buf.raw.chunks_exact(4).take(buf.len) {
                        let s = u32::from_le_bytes(chunk.try_into().unwrap());
                        v.push((((s as f64) / 2147483647.5) - 1.0) as f32);
                    }
                }
                Elem::U8 => {
                    for &b in buf.raw.iter().take(buf.len) {
                        v.push(((b as f32) - 128.0) / 128.0);
                    }
                }
            }
            v
        }
        Value::Bytes(raw) => {
            if (raw.len() & 1) != 0 {
                return Err(err_bad_opt("from_samples(bytes) expects even PCM16 byte length"));
            }
            raw.chunks_exact(2)
                .map(|c| (i16::from_le_bytes([c[0], c[1]]) as f32) / 32768.0)
                .collect()
        }
        other => {
            return Err(err_bad_opt(format!(
                "audio.from_samples() expects list, buffer, or bytes, got '{}'",
                other.type_name()
            )));
        }
    };

    let snd = from_samples_f32(
        &samples,
        sample_rate,
        channels,
        Some(vm.budget_core()),
        Some(&|| vm.check_timeout()),
    )?;
    Ok(Value::Sound(Rc::new(snd)))
}

fn audio_tone(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err(err_bad_opt("audio.tone() expects (freq_hz, ms, [opts])"));
    }
    let freq_hz = num_as_f64(&args[0], "audio.tone() freq_hz")?;
    let ms = num_as_u32(&args[1], "audio.tone() ms")?;
    let mut opts = ToneOpts::default();
    if let Some(arg2) = args.get(2) {
        if !matches!(arg2, Value::Unit) {
            let Value::Dict(dict) = arg2 else {
                return Err(err_bad_opt("audio.tone() opts must be dict"));
            };
            for (k, v) in dict.borrow().iter() {
                match k.as_str() {
                    "waveform" | "wave" | "shape" => {
                        let s = v.as_str().ok_or_else(|| {
                            err_bad_opt("audio.tone() waveform must be str")
                        })?;
                        opts.waveform = Waveform::from_name(s)?;
                    }
                    "volume" | "gain" => opts.volume = num_as_f32(v, "tone.volume")?,
                    "pan" => opts.pan = num_as_f32(v, "tone.pan")?,
                    "attack_ms" => opts.attack_ms = num_as_u32(v, "tone.attack_ms")?,
                    "decay_ms" => opts.decay_ms = num_as_u32(v, "tone.decay_ms")?,
                    "sustain" => opts.sustain = num_as_f32(v, "tone.sustain")?,
                    "release_ms" => opts.release_ms = num_as_u32(v, "tone.release_ms")?,
                    other => {
                        return Err(err_bad_opt(format!(
                            "audio.tone() unknown option '{}'",
                            other
                        )));
                    }
                }
            }
        }
    }
    let snd = synth_tone(
        freq_hz,
        ms,
        &opts,
        Some(vm.budget_core()),
        Some(&|| vm.check_timeout()),
    )?;
    Ok(Value::Sound(Rc::new(snd)))
}

fn audio_noise(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("audio.noise() expects (ms, [opts])"));
    }
    let ms = num_as_u32(&args[0], "audio.noise() ms")?;
    let mut opts = NoiseOpts::default();
    if let Some(arg1) = args.get(1) {
        if !matches!(arg1, Value::Unit) {
            let Value::Dict(dict) = arg1 else {
                return Err(err_bad_opt("audio.noise() opts must be dict"));
            };
            for (k, v) in dict.borrow().iter() {
                match k.as_str() {
                    "color" | "type" | "kind" => {
                        let s = v.as_str().ok_or_else(|| {
                            err_bad_opt("audio.noise() color must be str")
                        })?;
                        opts.color = NoiseColor::from_name(s)?;
                    }
                    "seed" => opts.seed = num_as_u64(v, "noise.seed")?,
                    "volume" | "gain" => opts.volume = num_as_f32(v, "noise.volume")?,
                    "pan" => opts.pan = num_as_f32(v, "noise.pan")?,
                    "attack_ms" => opts.attack_ms = num_as_u32(v, "noise.attack_ms")?,
                    "release_ms" => opts.release_ms = num_as_u32(v, "noise.release_ms")?,
                    other => {
                        return Err(err_bad_opt(format!(
                            "audio.noise() unknown option '{}'",
                            other
                        )));
                    }
                }
            }
        }
    }
    let snd = synth_noise(
        ms,
        &opts,
        Some(vm.budget_core()),
        Some(&|| vm.check_timeout()),
    )?;
    Ok(Value::Sound(Rc::new(snd)))
}

fn audio_render_offline(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("audio.render_offline() expects (sound, [opts])"));
    }
    let Value::Sound(snd) = &args[0] else {
        return Err(err_bad_opt(format!(
            "audio.render_offline() expects sound, got '{}'",
            args[0].type_name()
        )));
    };
    let mut volume = 1.0f32;
    let mut pan = 0.0f32;
    let mut pitch = 1.0f32;
    let mut loop_mode = snd.loop_points.is_some();
    let mut loop_points = snd.loop_points;
    let mut duration_ms: Option<u32> = None;
    let mut return_pcm = false;

    if let Some(arg1) = args.get(1) {
        if !matches!(arg1, Value::Unit) {
            let Value::Dict(dict) = arg1 else {
                return Err(err_bad_opt("audio.render_offline() opts must be dict"));
            };
            let map = dict.borrow();
            if let Some(v) = map.get("volume") {
                volume = num_as_f32(v, "render_offline.volume")?;
            }
            if let Some(v) = map.get("pan") {
                pan = num_as_f32(v, "render_offline.pan")?;
            }
            if let Some(v) = map.get("pitch") {
                pitch = num_as_f32(v, "render_offline.pitch")?;
            }
            if let Some(v) = map.get("loop").or_else(|| map.get("looped")) {
                if let Value::Bool(b) = v {
                    loop_mode = *b;
                }
            }
            if let Some(v) = map.get("duration_ms").or_else(|| map.get("ms")) {
                if !matches!(v, Value::Unit) {
                    duration_ms = Some(num_as_u32(v, "render_offline.duration_ms")?);
                }
            }
            if let Some(v) = map.get("format") {
                let s = v.as_str().ok_or_else(|| err_bad_opt("format must be str"))?;
                match s.to_ascii_lowercase().as_str() {
                    "wav" => return_pcm = false,
                    "pcm" | "f32" | "samples" => return_pcm = true,
                    other => {
                        return Err(err_bad_opt(format!(
                            "render_offline() unknown format '{}' (use 'wav' or 'pcm')",
                            other
                        )));
                    }
                }
            }
            drop(map);
            if let Some(lp) = parse_loop_override_opt(args.get(1))? {
                loop_points = Some(lp);
                loop_mode = true;
            }
        }
    }

    let pcm_src = snd.pcm_clone()?;
    let stereo = render_sound_offline(
        &pcm_src,
        snd.frames,
        volume,
        pan,
        pitch,
        loop_mode,
        loop_points,
        duration_ms,
    )?;
    if return_pcm {
        vm.charge_alloc(stereo.len().saturating_mul(std::mem::size_of::<Value>()))?;
        let vals = stereo.into_iter().map(|s| Value::Float(s as f64)).collect();
        Ok(Value::List(Rc::new(RefCell::new(vals))))
    } else {
        let wav = encode_wav_s16le(&stereo, ENGINE_RATE);
        vm.charge_alloc(wav.len())?;
        Ok(Value::Bytes(Rc::new(wav)))
    }
}

fn resolve_bus_id(val: &Value, ctx: &str) -> Result<u16, String> {
    match val {
        Value::Str(name) => get_or_create_bus(name),
        Value::Int(i) if *i >= 0 && *i <= (MAX_BUSES as i64) => {
            let id = *i as u16;
            if bus_exists(id) {
                Ok(id)
            } else {
                Err(err_bad_opt(format!("{} unknown bus id {}", ctx, id)))
            }
        }
        Value::StructInstance(inst) if inst.def.name == "AudioBus" => {
            let id = extract_handle_id(val, "AudioBus", ctx)?;
            let id16 = u16::try_from(id).map_err(|_| err_bad_opt("invalid bus id"))?;
            if bus_exists(id16) {
                Ok(id16)
            } else {
                Err(err_bad_opt(format!("{} unknown bus id {}", ctx, id16)))
            }
        }
        other => Err(err_bad_opt(format!(
            "{} expects AudioBus, bus name str, or int id, got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

fn apply_bus_effect_val(bus_id: u16, effect: &Value) -> Result<(), String> {
    let lowpass_hz = match effect {
        Value::Unit | Value::Bool(false) => None,
        Value::Str(s) if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("off") => None,
        Value::Int(_) | Value::Float(_) => Some(num_as_f32(effect, "bus_set_effect()")?),
        Value::Dict(d) => {
            let map = d.borrow();
            if let Some(hz) = map.get("cutoff_hz").or_else(|| map.get("lowpass_hz")) {
                Some(num_as_f32(hz, "bus_set_effect().cutoff_hz")?)
            } else if let Some(kind) = map.get("type").or_else(|| map.get("kind")) {
                let s = kind.as_str().ok_or_else(|| err_bad_opt("effect type must be str"))?;
                if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("off") {
                    None
                } else if s.eq_ignore_ascii_case("lowpass") {
                    let hz = map.get("cutoff").or_else(|| map.get("hz"));
                    match hz {
                        Some(v) => Some(num_as_f32(v, "lowpass cutoff")?),
                        None => Some(1200.0),
                    }
                } else {
                    return Err(err_bad_opt(format!("unsupported bus effect '{}'", s)));
                }
            } else {
                return Err(err_bad_opt(
                    "bus effect dict expects { type: 'lowpass', cutoff_hz: hz }",
                ));
            }
        }
        other => {
            return Err(err_bad_opt(format!(
                "bus_set_effect() invalid effect arg '{}'",
                other.type_name()
            )));
        }
    };
    send_command(MixerCommand::SetBusEffect { bus_id, lowpass_hz })
}

fn audio_bus(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("audio.bus() expects (name_or_id)"));
    }
    let bid = resolve_bus_id(&args[0], "audio.bus()")?;
    Ok(make_bus_handle(bid))
}

fn audio_bus_new(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("audio.bus_new() expects (name, [opts])"));
    }
    let name = args[0]
        .as_str()
        .ok_or_else(|| err_bad_opt("audio.bus_new() name must be str"))?;
    let bid = get_or_create_bus(name)?;
    if let Some(arg1) = args.get(1) {
        if !matches!(arg1, Value::Unit) {
            let Value::Dict(dict) = arg1 else {
                return Err(err_bad_opt("audio.bus_new() opts must be dict"));
            };
            for (k, v) in dict.borrow().iter() {
                match k.as_str() {
                    "volume" | "gain" => {
                        let vol = num_as_f32(v, "bus_new.volume")?;
                        send_command(MixerCommand::SetBusVolume {
                            bus_id: bid,
                            volume: vol,
                        })?;
                    }
                    "mute" | "muted" => {
                        let Value::Bool(m) = v else {
                            return Err(err_bad_opt("bus_new.muted must be bool"));
                        };
                        send_command(MixerCommand::SetBusMute {
                            bus_id: bid,
                            muted: *m,
                        })?;
                    }
                    "lowpass" | "effect" => apply_bus_effect_val(bid, v)?,
                    other => {
                        return Err(err_bad_opt(format!(
                            "audio.bus_new() unknown option '{}'",
                            other
                        )));
                    }
                }
            }
        }
    }
    Ok(make_bus_handle(bid))
}

fn audio_bus_name(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("bus.name() expects (bus)"));
    }
    let bid = resolve_bus_id(&args[0], "bus.name()")?;
    let (name, _, _) = get_bus_state(bid)
        .ok_or_else(|| err_bad_opt(format!("bus {} not found", bid)))?;
    Ok(Value::from_str(name))
}

fn audio_bus_volume(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("bus.volume() expects (bus)"));
    }
    let bid = resolve_bus_id(&args[0], "bus.volume()")?;
    let (_, vol, _) = get_bus_state(bid)
        .ok_or_else(|| err_bad_opt(format!("bus {} not found", bid)))?;
    Ok(Value::Float(vol as f64))
}

fn audio_bus_muted(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("bus.muted() expects (bus)"));
    }
    let bid = resolve_bus_id(&args[0], "bus.muted()")?;
    let (_, _, muted) = get_bus_state(bid)
        .ok_or_else(|| err_bad_opt(format!("bus {} not found", bid)))?;
    Ok(Value::Bool(muted))
}

fn audio_bus_set_volume(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("audio.bus_set_volume() expects (bus, volume)"));
    }
    let bid = resolve_bus_id(&args[0], "bus_set_volume()")?;
    let vol = num_as_f32(&args[1], "bus_set_volume() volume")?;
    send_command(MixerCommand::SetBusVolume {
        bus_id: bid,
        volume: vol,
    })?;
    Ok(Value::Unit)
}

fn audio_bus_set_mute(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("audio.bus_set_mute() expects (bus, bool)"));
    }
    let bid = resolve_bus_id(&args[0], "bus_set_mute()")?;
    let Value::Bool(muted) = &args[1] else {
        return Err(err_bad_opt("audio.bus_set_mute() muted must be bool"));
    };
    send_command(MixerCommand::SetBusMute {
        bus_id: bid,
        muted: *muted,
    })?;
    Ok(Value::Unit)
}

fn audio_bus_set_effect(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("audio.bus_set_effect() expects (bus, effect)"));
    }
    let bid = resolve_bus_id(&args[0], "bus_set_effect()")?;
    apply_bus_effect_val(bid, &args[1])?;
    Ok(Value::Unit)
}

fn audio_bus_stop_all(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("bus.stop_all() expects (bus)"));
    }
    let bid = resolve_bus_id(&args[0], "bus.stop_all()")?;
    send_command(MixerCommand::StopBusVoices { bus_id: Some(bid) })?;
    Ok(Value::Unit)
}

struct PlayOpts {
    bus_id: u16,
    volume: f32,
    pan: f32,
    pitch: f32,
    loop_mode: bool,
    loop_points: Option<(u64, u64)>,
    fade_in_ms: u32,
    priority: i32,
    on_full: OverflowPolicy,
    pos3: Option<[f32; 3]>,
}

impl Default for PlayOpts {
    fn default() -> Self {
        Self {
            bus_id: 0,
            volume: 1.0,
            pan: 0.0,
            pitch: 1.0,
            loop_mode: false,
            loop_points: None,
            fade_in_ms: 0,
            priority: 0,
            on_full: OverflowPolicy::Error,
            pos3: None,
        }
    }
}

fn parse_play_opts(val: Option<&Value>, default_loop: Option<(u64, u64)>) -> Result<PlayOpts, String> {
    let mut out = PlayOpts {
        loop_mode: default_loop.is_some(),
        loop_points: default_loop,
        ..PlayOpts::default()
    };
    let Some(v) = val else {
        return Ok(out);
    };
    if matches!(v, Value::Unit) {
        return Ok(out);
    }
    let Value::Dict(dict) = v else {
        return Err(err_bad_opt("audio.play() opts must be dict"));
    };
    for (k, fv) in dict.borrow().iter() {
        match k.as_str() {
            "bus" => out.bus_id = resolve_bus_id(fv, "play().bus")?,
            "volume" | "gain" => out.volume = num_as_f32(fv, "play().volume")?,
            "pan" => out.pan = num_as_f32(fv, "play().pan")?,
            "pitch" | "rate" => out.pitch = num_as_f32(fv, "play().pitch")?,
            "loop" | "looped" => {
                let Value::Bool(b) = fv else {
                    return Err(err_bad_opt("play().loop must be bool"));
                };
                out.loop_mode = *b;
            }
            "fade_in_ms" | "fade_ms" => {
                out.fade_in_ms = num_as_u32(fv, "play().fade_in_ms")?.min(MAX_FADE_MS);
            }
            "priority" => {
                let Value::Int(p) = fv else {
                    return Err(err_bad_opt("play().priority must be int"));
                };
                out.priority = (*p).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            }
            "on_full" | "overflow" => {
                let s = fv.as_str().ok_or_else(|| err_bad_opt("play().on_full must be str"))?;
                match s.trim().to_ascii_lowercase().as_str() {
                    "error" => out.on_full = OverflowPolicy::Error,
                    "drop_oldest" | "drop" | "steal" => out.on_full = OverflowPolicy::DropOldest,
                    other => {
                        return Err(err_bad_opt(format!(
                            "play().on_full unknown policy '{}' (use 'error' or 'drop_oldest')",
                            other
                        )));
                    }
                }
            }
            "pos3" | "pos" => {
                out.pos3 = Some(extract_vec3(fv, "play().pos3")?);
            }
            "loop_points" | "loop_start" | "loop_end" => {}
            other => {
                return Err(err_bad_opt(format!("audio.play() unknown option '{}'", other)));
            }
        }
    }
    if let Some(lp) = parse_loop_override_opt(val)? {
        out.loop_points = Some(lp);
        out.loop_mode = true;
    }
    Ok(out)
}

fn play_music_inner(mid: u64, opts: Option<&Value>) -> Result<Value, String> {
    let mut bus_id = 0u16;
    let mut vol_override = None;
    let mut fade_in_ms = 0u32;
    if let Some(v) = opts {
        if !matches!(v, Value::Unit) {
            let Value::Dict(dict) = v else {
                return Err(err_bad_opt("music.play() opts must be dict"));
            };
            for (k, fv) in dict.borrow().iter() {
                match k.as_str() {
                    "bus" => bus_id = resolve_bus_id(fv, "music.play().bus")?,
                    "volume" | "gain" => vol_override = Some(num_as_f32(fv, "music.play().volume")?),
                    "fade_in_ms" | "fade_ms" => {
                        fade_in_ms = num_as_u32(fv, "music.play().fade_in_ms")?.min(MAX_FADE_MS);
                    }
                    other => {
                        return Err(err_bad_opt(format!("music.play() unknown option '{}'", other)));
                    }
                }
            }
        }
    }
    let vid = play_music_stream(mid, bus_id, vol_override, fade_in_ms)?;
    Ok(make_voice_handle(vid))
}

fn audio_play(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("audio.play() expects (sound|music|path, [opts])"));
    }
    match &args[0] {
        // Legacy compatibility path: `audio.play(path)` gated only by read sandbox, auto-inits engine.
        Value::Str(path) => {
            if args.len() > 1 {
                vm.check_audio()?;
            }
            let (bytes, _, _) = read_asset_bytes(vm, path)?;
            let snd = decode_wav_sound(
                &bytes,
                None,
                Some(vm.budget_core()),
                Some(&|| vm.check_timeout()),
            )?;
            ensure_initialized_auto(
                vm.is_deterministic(),
                Some(Arc::clone(vm.budget_core())),
                vm.remaining_timeout_ms(),
            )?;
            let po = parse_play_opts(args.get(1), snd.loop_points)?;
            let pcm = snd.pcm_clone()?;
            let vid = play_pcm_voice(
                pcm,
                snd.frames,
                po.bus_id,
                po.volume,
                po.pan,
                po.pitch,
                po.loop_mode,
                po.loop_points,
                po.fade_in_ms,
                po.priority,
                po.on_full,
                po.pos3,
            )?;
            Ok(make_voice_handle(vid))
        }
        Value::Sound(snd) => {
            vm.check_audio()?;
            ensure_initialized_auto(
                vm.is_deterministic(),
                Some(Arc::clone(vm.budget_core())),
                vm.remaining_timeout_ms(),
            )?;
            let po = parse_play_opts(args.get(1), snd.loop_points)?;
            let pcm = snd.pcm_clone()?;
            let vid = play_pcm_voice(
                pcm,
                snd.frames,
                po.bus_id,
                po.volume,
                po.pan,
                po.pitch,
                po.loop_mode,
                po.loop_points,
                po.fade_in_ms,
                po.priority,
                po.on_full,
                po.pos3,
            )?;
            Ok(make_voice_handle(vid))
        }
        Value::StructInstance(inst) if inst.def.name == "AudioMusic" => {
            vm.check_audio()?;
            let mid = extract_handle_id(&args[0], "AudioMusic", "audio.play()")?;
            update_deadline(vm.remaining_timeout_ms());
            play_music_inner(mid, args.get(1))
        }
        other => Err(err_bad_opt(format!(
            "audio.play() expects Sound, Music, or WAV path str, got '{}'",
            other.type_name()
        ))),
    }
}

fn audio_play_at(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() < 2 || args.len() > 5 {
        return Err(err_bad_opt(
            "audio.play_at() expects (sound, vec3|[x,y,z], [opts]) or (sound, x, y, z, [opts])",
        ));
    }
    let Value::Sound(snd) = &args[0] else {
        return Err(err_bad_opt(format!(
            "audio.play_at() expects Sound, got '{}'",
            args[0].type_name()
        )));
    };
    let (pos3, opts_val) = match &args[1] {
        Value::Vec3(_) | Value::List(_) => {
            if args.len() > 3 {
                return Err(err_bad_opt(
                    "audio.play_at(sound, pos3, [opts]) expects at most 3 args",
                ));
            }
            (extract_vec3(&args[1], "audio.play_at()")?, args.get(2))
        }
        _ => {
            if args.len() < 4 {
                return Err(err_bad_opt(
                    "audio.play_at(sound, x, y, z, [opts]) expects x, y, z",
                ));
            }
            let x = num_as_f32(&args[1], "audio.play_at().x")?;
            let y = num_as_f32(&args[2], "audio.play_at().y")?;
            let z = num_as_f32(&args[3], "audio.play_at().z")?;
            ([x, y, z], args.get(4))
        }
    };
    ensure_initialized_auto(
        vm.is_deterministic(),
        Some(Arc::clone(vm.budget_core())),
        vm.remaining_timeout_ms(),
    )?;
    let po = parse_play_opts(opts_val, snd.loop_points)?;
    let pcm = snd.pcm_clone()?;
    let vid = play_pcm_voice(
        pcm,
        snd.frames,
        po.bus_id,
        po.volume,
        po.pan,
        po.pitch,
        po.loop_mode,
        po.loop_points,
        po.fade_in_ms,
        po.priority,
        po.on_full,
        Some(pos3),
    )?;
    Ok(make_voice_handle(vid))
}

fn audio_stop(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        // Legacy no-arg `audio.stop()` stops master playback safely even in sandbox mode.
        if is_initialized() {
            send_command(MixerCommand::StopBusVoices { bus_id: None })?;
        }
        return Ok(Value::Unit);
    }
    if args.len() != 1 {
        return Err(err_bad_opt("audio.stop() expects 0 or 1 argument"));
    }
    vm.check_audio()?;
    match &args[0] {
        Value::StructInstance(inst) if inst.def.name == "AudioMusic" => audio_music_stop(vm, args),
        Value::StructInstance(inst) if inst.def.name == "AudioBus" => audio_bus_stop_all(vm, args),
        _ => audio_voice_stop(vm, args),
    }
}

fn audio_stop_all(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() > 1 {
        return Err(err_bad_opt("audio.stop_all() expects 0 or 1 (bus) argument"));
    }
    let bus_id = match args.first() {
        None | Some(Value::Unit) => None,
        Some(v) => Some(resolve_bus_id(v, "audio.stop_all()")?),
    };
    if is_initialized() {
        send_command(MixerCommand::StopBusVoices { bus_id })?;
    }
    Ok(Value::Unit)
}

fn audio_pause_all(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() > 1 {
        return Err(err_bad_opt("audio.pause_all() expects 0 or 1 (bus) argument"));
    }
    let bus_id = match args.first() {
        None | Some(Value::Unit) => None,
        Some(v) => Some(resolve_bus_id(v, "audio.pause_all()")?),
    };
    send_command(MixerCommand::PauseBusVoices {
        bus_id,
        paused: true,
    })?;
    Ok(Value::Unit)
}

fn audio_resume_all(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() > 1 {
        return Err(err_bad_opt("audio.resume_all() expects 0 or 1 (bus) argument"));
    }
    let bus_id = match args.first() {
        None | Some(Value::Unit) => None,
        Some(v) => Some(resolve_bus_id(v, "audio.resume_all()")?),
    };
    send_command(MixerCommand::PauseBusVoices {
        bus_id,
        paused: false,
    })?;
    Ok(Value::Unit)
}

fn audio_set_listener(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.is_empty() || args.len() > 4 {
        return Err(err_bad_opt(
            "audio.set_listener() expects (opts_dict) or (pos3, [forward, rolloff, max_dist])",
        ));
    }
    let mut pos = [0.0f32, 0.0, 0.0];
    let mut forward = [0.0f32, 0.0, -1.0];
    let mut rolloff = 1.0f32;
    let mut max_dist = 64.0f32;

    if let Value::Dict(dict) = &args[0] {
        if args.len() != 1 {
            return Err(err_bad_opt("audio.set_listener(opts_dict) expects 1 arg"));
        }
        for (k, v) in dict.borrow().iter() {
            match k.as_str() {
                "pos" | "pos3" | "position" => pos = extract_vec3(v, "set_listener.pos")?,
                "forward" | "dir" => forward = extract_vec3(v, "set_listener.forward")?,
                "rolloff" => rolloff = num_as_f32(v, "set_listener.rolloff")?,
                "max_dist" | "max_distance" => {
                    max_dist = num_as_f32(v, "set_listener.max_dist")?;
                }
                other => {
                    return Err(err_bad_opt(format!(
                        "audio.set_listener() unknown option '{}'",
                        other
                    )));
                }
            }
        }
    } else {
        pos = extract_vec3(&args[0], "audio.set_listener().pos")?;
        if let Some(a1) = args.get(1) {
            if !matches!(a1, Value::Unit) {
                forward = extract_vec3(a1, "audio.set_listener().forward")?;
            }
        }
        if let Some(a2) = args.get(2) {
            if !matches!(a2, Value::Unit) {
                rolloff = num_as_f32(a2, "audio.set_listener().rolloff")?;
            }
        }
        if let Some(a3) = args.get(3) {
            if !matches!(a3, Value::Unit) {
                max_dist = num_as_f32(a3, "audio.set_listener().max_dist")?;
            }
        }
    }
    send_command(MixerCommand::SetListener {
        pos,
        forward,
        rolloff,
        max_dist,
    })?;
    Ok(Value::Unit)
}

fn audio_clock_ms(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if !args.is_empty() {
        return Err(err_bad_opt("audio.clock_ms() expects 0 arguments"));
    }
    let st = get_state();
    Ok(Value::Int(st.clock_ms as i64))
}

fn audio_tick(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    vm.check_timeout()?;
    if args.len() != 1 {
        return Err(err_bad_opt("audio.tick() expects (ms)"));
    }
    let ms = num_as_u32(&args[0], "audio.tick() ms")?;
    let (_frames, clock_ms) = tick_virtual(ms, vm.is_deterministic())?;
    Ok(Value::Int(clock_ms as i64))
}

fn audio_voice_stop(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("voice.stop() expects (voice)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.stop()")?;
    send_command(MixerCommand::StopVoice { voice_id: vid })?;
    Ok(Value::Unit)
}

fn audio_voice_pause(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("voice.pause() expects (voice)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.pause()")?;
    send_command(MixerCommand::PauseVoice { voice_id: vid })?;
    Ok(Value::Unit)
}

fn audio_voice_resume(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("voice.resume() expects (voice)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.resume()")?;
    send_command(MixerCommand::ResumeVoice { voice_id: vid })?;
    Ok(Value::Unit)
}

fn audio_voice_set_volume(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("voice.set_volume() expects (voice, volume)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.set_volume()")?;
    let volume = num_as_f32(&args[1], "voice.set_volume()")?;
    send_command(MixerCommand::SetVoiceVolume {
        voice_id: vid,
        volume,
    })?;
    Ok(Value::Unit)
}

fn audio_voice_set_pan(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("voice.set_pan() expects (voice, pan)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.set_pan()")?;
    let pan = num_as_f32(&args[1], "voice.set_pan()")?;
    send_command(MixerCommand::SetVoicePan { voice_id: vid, pan })?;
    Ok(Value::Unit)
}

fn audio_voice_set_pitch(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("voice.set_pitch() expects (voice, pitch)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.set_pitch()")?;
    let pitch = num_as_f32(&args[1], "voice.set_pitch()")?;
    send_command(MixerCommand::SetVoicePitch {
        voice_id: vid,
        pitch,
    })?;
    Ok(Value::Unit)
}

fn audio_voice_set_loop(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("voice.set_loop() expects (voice, bool)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.set_loop()")?;
    let Value::Bool(loop_mode) = &args[1] else {
        return Err(err_bad_opt("voice.set_loop() expects bool"));
    };
    send_command(MixerCommand::SetVoiceLoop {
        voice_id: vid,
        loop_mode: *loop_mode,
    })?;
    Ok(Value::Unit)
}

fn audio_voice_set_loop_points(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 3 {
        return Err(err_bad_opt(
            "voice.set_loop_points() expects (voice, start_frame, end_frame)",
        ));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.set_loop_points()")?;
    let start_frame = num_as_u64(&args[1], "set_loop_points().start_frame")?;
    let end_frame = num_as_u64(&args[2], "set_loop_points().end_frame")?;
    if start_frame >= end_frame {
        return Err(err_bad_opt("loop start_frame must be < end_frame"));
    }
    send_command(MixerCommand::SetVoiceLoopPoints {
        voice_id: vid,
        start_frame,
        end_frame,
    })?;
    Ok(Value::Unit)
}

fn audio_voice_fade_to(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() < 3 || args.len() > 4 {
        return Err(err_bad_opt(
            "voice.fade_to() expects (voice, target_vol, ms, [stop_on_finish])",
        ));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.fade_to()")?;
    let target_vol = num_as_f32(&args[1], "fade_to().target_vol")?;
    let ms = num_as_u32(&args[2], "fade_to().ms")?.min(MAX_FADE_MS);
    let stop_on_finish = match args.get(3) {
        None | Some(Value::Unit) => false,
        Some(Value::Bool(b)) => *b,
        Some(other) => {
            return Err(err_bad_opt(format!(
                "fade_to() stop_on_finish must be bool, got '{}'",
                other.type_name()
            )));
        }
    };
    send_command(MixerCommand::FadeVoice {
        voice_id: vid,
        target_vol,
        ms,
        stop_on_finish,
    })?;
    Ok(Value::Unit)
}

fn audio_voice_set_pos3(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    let (vid, pos3) = if args.len() == 2 {
        (
            extract_handle_id(&args[0], "AudioVoice", "voice.set_pos3()")?,
            extract_vec3(&args[1], "voice.set_pos3()")?,
        )
    } else if args.len() == 4 {
        (
            extract_handle_id(&args[0], "AudioVoice", "voice.set_pos3()")?,
            [
                num_as_f32(&args[1], "set_pos3().x")?,
                num_as_f32(&args[2], "set_pos3().y")?,
                num_as_f32(&args[3], "set_pos3().z")?,
            ],
        )
    } else {
        return Err(err_bad_opt(
            "voice.set_pos3() expects (voice, pos3) or (voice, x, y, z)",
        ));
    };
    send_command(MixerCommand::SetVoicePos3 {
        voice_id: vid,
        pos3,
    })?;
    Ok(Value::Unit)
}

fn audio_voice_is_playing(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("voice.is_playing() expects (voice)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.is_playing()")?;
    let playing = get_voice_shared(vid)
        .map(|s| s.playing.load(Ordering::Acquire))
        .unwrap_or(false);
    Ok(Value::Bool(playing))
}

fn audio_voice_position_ms(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("voice.position_ms() expects (voice)"));
    }
    let vid = extract_handle_id(&args[0], "AudioVoice", "voice.position_ms()")?;
    let pos = get_voice_shared(vid)
        .map(|s| s.pos_ms.load(Ordering::Acquire))
        .unwrap_or(0);
    Ok(Value::Int(pos as i64))
}

fn audio_music_play(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.is_empty() || args.len() > 2 {
        return Err(err_bad_opt("music.play() expects (music, [opts])"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.play()")?;
    update_deadline(vm.remaining_timeout_ms());
    play_music_inner(mid, args.get(1))
}

fn audio_music_stop(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("music.stop() expects (music)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.stop()")?;
    control_music(mid, |slot, cmds| {
        if let Some(vid) = slot.active_voice_id.take() {
            cmds.try_send(MixerCommand::StopVoice { voice_id: vid })?;
        }
        slot.shared
            .state
            .store(MusicStatus::Idle as u8, Ordering::Release);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn audio_music_pause(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("music.pause() expects (music)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.pause()")?;
    control_music(mid, |slot, cmds| {
        if let Some(vid) = slot.active_voice_id {
            cmds.try_send(MixerCommand::PauseVoice { voice_id: vid })?;
        }
        slot.shared
            .state
            .store(MusicStatus::Paused as u8, Ordering::Release);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn audio_music_resume(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("music.resume() expects (music)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.resume()")?;
    control_music(mid, |slot, cmds| {
        if let Some(vid) = slot.active_voice_id {
            cmds.try_send(MixerCommand::ResumeVoice { voice_id: vid })?;
        }
        slot.shared
            .state
            .store(MusicStatus::Playing as u8, Ordering::Release);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn audio_music_seek(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("music.seek() expects (music, ms)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.seek()")?;
    let ms = num_as_u64(&args[1], "music.seek() ms")?;
    control_music(mid, |slot, _cmds| {
        let frame = ((ms.saturating_mul(ENGINE_RATE as u64)) / 1000).min(slot.total_frames);
        if let Ok(mut rg) = slot.ring.lock() {
            rg.clear_for_seek(frame);
        }
        let clamped_ms = (frame.saturating_mul(1000)) / (ENGINE_RATE as u64);
        slot.shared.pos_ms.store(clamped_ms, Ordering::Release);
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn audio_music_position_ms(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("music.position_ms() expects (music)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.position_ms()")?;
    let pos = get_music_shared(mid)
        .map(|(s, _)| s.pos_ms.load(Ordering::Acquire))
        .unwrap_or(0);
    Ok(Value::Int(pos as i64))
}

fn audio_music_duration_ms(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("music.duration_ms() expects (music)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.duration_ms()")?;
    let dur = get_music_shared(mid)
        .map(|(s, _)| s.dur_ms.load(Ordering::Acquire))
        .unwrap_or(0);
    Ok(Value::Int(dur as i64))
}

fn audio_music_set_volume(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("music.set_volume() expects (music, volume)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.set_volume()")?;
    let volume = num_as_f32(&args[1], "music.set_volume()")?.clamp(0.0, 4.0);
    control_music(mid, |slot, cmds| {
        slot.volume = volume;
        slot.shared
            .vol_milli
            .store((volume * 1000.0) as u32, Ordering::Release);
        if let Some(vid) = slot.active_voice_id {
            cmds.try_send(MixerCommand::SetVoiceVolume {
                voice_id: vid,
                volume,
            })?;
        }
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn audio_music_set_loop(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 2 {
        return Err(err_bad_opt("music.set_loop() expects (music, bool)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.set_loop()")?;
    let Value::Bool(loop_mode) = &args[1] else {
        return Err(err_bad_opt("music.set_loop() expects bool"));
    };
    control_music(mid, |slot, cmds| {
        slot.loop_mode = *loop_mode;
        if let Some(vid) = slot.active_voice_id {
            cmds.try_send(MixerCommand::SetVoiceLoop {
                voice_id: vid,
                loop_mode: *loop_mode,
            })?;
        }
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn audio_music_set_loop_points(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 3 {
        return Err(err_bad_opt(
            "music.set_loop_points() expects (music, start_frame, end_frame)",
        ));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.set_loop_points()")?;
    let start_frame = num_as_u64(&args[1], "music.set_loop_points().start_frame")?;
    let end_frame = num_as_u64(&args[2], "music.set_loop_points().end_frame")?;
    if start_frame >= end_frame {
        return Err(err_bad_opt("loop start_frame must be < end_frame"));
    }
    control_music(mid, |slot, cmds| {
        slot.loop_points = Some((start_frame, end_frame));
        slot.loop_mode = true;
        if let Some(vid) = slot.active_voice_id {
            cmds.try_send(MixerCommand::SetVoiceLoopPoints {
                voice_id: vid,
                start_frame,
                end_frame,
            })?;
        }
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn audio_music_fade_to(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() < 3 || args.len() > 4 {
        return Err(err_bad_opt(
            "music.fade_to() expects (music, target_vol, ms, [stop_on_finish])",
        ));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.fade_to()")?;
    let target_vol = num_as_f32(&args[1], "music.fade_to().target_vol")?;
    let ms = num_as_u32(&args[2], "music.fade_to().ms")?.min(MAX_FADE_MS);
    let stop_on_finish = match args.get(3) {
        None | Some(Value::Unit) => false,
        Some(Value::Bool(b)) => *b,
        _ => return Err(err_bad_opt("music.fade_to() stop_on_finish must be bool")),
    };
    control_music(mid, |slot, cmds| {
        if let Some(vid) = slot.active_voice_id {
            cmds.try_send(MixerCommand::FadeVoice {
                voice_id: vid,
                target_vol,
                ms,
                stop_on_finish,
            })?;
        } else {
            slot.volume = target_vol.clamp(0.0, 4.0);
        }
        Ok(())
    })?;
    Ok(Value::Unit)
}

fn audio_music_state(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("music.state() expects (music)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.state()")?;
    let st = get_music_shared(mid)
        .map(|(s, _)| MusicStatus::from_u8(s.state.load(Ordering::Acquire)).name())
        .unwrap_or("closed");
    Ok(Value::from_str(st))
}

fn audio_music_close(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_audio()?;
    if args.len() != 1 {
        return Err(err_bad_opt("music.close() expects (music)"));
    }
    let mid = extract_handle_id(&args[0], "AudioMusic", "music.close()")?;
    close_music_stream(mid);
    Ok(Value::Unit)
}

fn expect_sound(arg: &Value, method: &str) -> Result<Rc<SoundData>, String> {
    match arg {
        Value::Sound(s) => Ok(Rc::clone(s)),
        other => Err(err_bad_opt(format!(
            "{} expects Sound, got '{}'",
            method,
            other.type_name()
        ))),
    }
}

pub(crate) fn snd_duration_ms(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("sound.duration_ms() expects 0 arguments"));
    }
    let snd = expect_sound(&args[0], "sound.duration_ms()")?;
    Ok(Value::Int(snd.duration_ms() as i64))
}

pub(crate) fn snd_sample_rate(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("sound.sample_rate() expects 0 arguments"));
    }
    let _ = expect_sound(&args[0], "sound.sample_rate()")?;
    Ok(Value::Int(ENGINE_RATE as i64))
}

pub(crate) fn snd_channels(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("sound.channels() expects 0 arguments"));
    }
    let _ = expect_sound(&args[0], "sound.channels()")?;
    Ok(Value::Int(2))
}

pub(crate) fn snd_frames(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("sound.frames() expects 0 arguments"));
    }
    let snd = expect_sound(&args[0], "sound.frames()")?;
    Ok(Value::Int(snd.frames as i64))
}

pub(crate) fn snd_digest(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("sound.digest() expects 0 arguments"));
    }
    let snd = expect_sound(&args[0], "sound.digest()")?;
    Ok(Value::from_str(snd.digest.to_hex()))
}

pub(crate) fn snd_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(err_bad_opt("sound.close() expects 0 arguments"));
    }
    let snd = expect_sound(&args[0], "sound.close()")?;
    snd.close();
    Ok(Value::Unit)
}
