//! Display-thread JNI adapter for the shared visual scene. This path never
//! serializes business snapshots or sends per-node JNI calls on each frame.
use jni::{
    objects::{JByteBuffer, JFloatArray, JIntArray, JObject},
    sys::{jboolean, jdouble, jfloat, jint, jlong},
    EnvUnowned,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
use zork_liquid::{
    scene::{Scene, COMMAND_BYTES, MAX_COMMANDS, MAX_FRAME_BYTES},
    tokens,
};

#[derive(Debug)]
enum BridgeError {
    Jni(jni::errors::Error),
    Invalid(&'static str),
    Scene(zork_liquid::scene::Error),
}
impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jni(error) => error.fmt(f),
            Self::Invalid(message) => f.write_str(message),
            Self::Scene(error) => write!(f, "liquid frame: {error:?}"),
        }
    }
}
impl std::error::Error for BridgeError {}
impl From<jni::errors::Error> for BridgeError {
    fn from(error: jni::errors::Error) -> Self {
        Self::Jni(error)
    }
}
type Result<T> = std::result::Result<T, BridgeError>;
macro_rules! ensure {
    ($condition:expr, $message:literal) => {
        if !$condition {
            return Err(BridgeError::Invalid($message));
        }
    };
}

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
thread_local! { static SCENES: RefCell<HashMap<u64, Scene>> = RefCell::new(HashMap::new()); }

#[unsafe(no_mangle)]
pub extern "system" fn Java_ing_zork_android_LiquidNative_create<'a>(
    mut env: EnvUnowned<'a>,
    _this: JObject<'a>,
) -> jlong {
    env.with_env(|_| -> Result<jlong> {
        SCENES.with_borrow_mut(|scenes| {
            ensure!(scenes.len() < 32, "too many liquid hosts on this thread");
            let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
            ensure!(
                handle > 0 && handle <= i64::MAX as u64,
                "liquid handle exhausted"
            );
            scenes.insert(handle, Scene::default());
            Ok(handle as jlong)
        })
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_ing_zork_android_LiquidNative_destroy<'a>(
    mut env: EnvUnowned<'a>,
    _this: JObject<'a>,
    handle: jlong,
) {
    env.with_env(|_| -> Result<()> {
        SCENES.with_borrow_mut(|scenes| {
            scenes.remove(&(handle as u64));
        });
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

/// Return required bytes with a negative sign when the output needs to grow.
/// inputBytes=-1 drains the retained frame without applying or advancing twice.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ing_zork_android_LiquidNative_frame<'a>(
    mut env: EnvUnowned<'a>,
    _this: JObject<'a>,
    handle: jlong,
    input: JByteBuffer<'a>,
    input_bytes: jint,
    elapsed: jdouble,
    reduced: jboolean,
    output: JByteBuffer<'a>,
) -> jint {
    env.with_env(|env| -> Result<jint> {
        ensure!(
            (-1..=(COMMAND_BYTES * MAX_COMMANDS) as i32).contains(&input_bytes),
            "invalid liquid command length"
        );
        let capacity = env.get_direct_buffer_capacity(&output)?;
        ensure!(capacity <= MAX_FRAME_BYTES, "liquid output exceeds limit");
        let destination = env.get_direct_buffer_address(&output)?;
        // The UI thread exclusively owns these buffers for this synchronous
        // call. Copy commands before writing output, also safe for aliased views.
        let source = if input_bytes >= 0 {
            ensure!(
                input_bytes as usize <= env.get_direct_buffer_capacity(&input)?,
                "short liquid input"
            );
            Some(env.get_direct_buffer_address(&input)?)
        } else {
            None
        };
        SCENES.with_borrow_mut(|scenes| {
            let scene = scenes
                .get_mut(&(handle as u64))
                .ok_or(BridgeError::Invalid("invalid liquid host or wrong thread"))?;
            if let Some(source) = source {
                // No borrowed Java memory escapes this method. Scene parses
                // the entire input before it produces any output bytes.
                let bytes = unsafe { std::slice::from_raw_parts(source, input_bytes as usize) };
                scene
                    .frame(bytes, elapsed, reduced)
                    .map_err(BridgeError::Scene)?;
            }
            let bytes = scene.pending();
            if bytes.len() > capacity {
                return Ok(-(bytes.len() as jint));
            }
            // Source storage is Rust-owned and cannot alias the Java buffer.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
            }
            Ok(bytes.len() as jint)
        })
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_ing_zork_android_LiquidNative_palette<'a>(
    mut env: EnvUnowned<'a>,
    _this: JObject<'a>,
) -> JIntArray<'a> {
    env.with_env(|env| -> Result<_> {
        let values = [
            tokens::BRAND_ACCENT,
            tokens::LIQUID_OUTLINE,
            tokens::INTERACTION.neutral_hover,
            tokens::INTERACTION.neutral_pressed,
            tokens::INTERACTION.primary_hover,
            tokens::INTERACTION.primary_pressed,
            tokens::INTERACTION.accent_hover,
            tokens::INTERACTION.accent_pressed,
            tokens::INTERACTION.focus_border,
            tokens::BORDER_WIDTH.to_bits(),
            (tokens::SMOOTHING as f32).to_bits(),
            tokens::CARD_RADIUS.to_bits(),
            tokens::FIELD_RADIUS.to_bits(),
            tokens::COMPACT_CARD_RADIUS.to_bits(),
            tokens::BUTTON_RADIUS.to_bits(),
            tokens::ICON_BUTTON_RADIUS.to_bits(),
        ];
        let output = env.new_int_array(values.len())?;
        output.set_region(env, 0, &values.map(|v| v as jint))?;
        Ok(output)
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// Cold layout/clip path for platform Shape APIs. Callers cache by measured size
/// and radius; animation uses the batched scene instead of this entry point.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ing_zork_android_LiquidNative_geometry<'a>(
    mut env: EnvUnowned<'a>,
    _this: JObject<'a>,
    width: jfloat,
    height: jfloat,
    radius: jfloat,
) -> JFloatArray<'a> {
    env.with_env(|env| -> Result<_> {
        ensure!(
            [width, height, radius].into_iter().all(f32::is_finite)
                && width >= 2.
                && height >= 2.
                && width <= 65536.
                && height <= 65536.
                && radius >= 0.,
            "invalid liquid shape"
        );
        let curves = zork_liquid::rounded_rectangle(
            zork_liquid::Pose::rect(0., 0., width as f64, height as f64, radius as f64),
            tokens::SMOOTHING,
        );
        let mut values = Vec::with_capacity(2 + curves.len() * 6);
        values.extend(curves[0].from.map(|v| v as f32));
        for curve in curves {
            for p in [curve.c1, curve.c2, curve.to] {
                values.extend(p.map(|v| v as f32));
            }
        }
        let output = env.new_float_array(values.len())?;
        output.set_region(env, 0, &values)?;
        Ok(output)
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}
