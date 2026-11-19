// SPDX-License-Identifier: AGPL-3.0-or-later
//! Android-only JNI entry point, called once from `Application.onCreate` before any network use.
//!
//! rustls-platform-verifier checks certificates with Android's own trust manager (so
//! user-installed CAs work), which needs a JVM handle and the app context.

use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::jni_mangle;
use jni::objects::{JClass, JObject};

#[jni_mangle("be.alwyapps.watch.core.NativeTls")]
pub fn init<'caller>(
    mut unowned_env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    context: JObject<'caller>,
) {
    unowned_env
        .with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
        .resolve::<ThrowRuntimeExAndDefault>();
}
