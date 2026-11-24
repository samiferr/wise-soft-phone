// The account settings contain only the identifier for a platform-stored credential.
#[cfg(not(target_os = "android"))]
mod platform {
    fn entry(account: &str) -> Result<keyring::v1::Entry, ()> {
        keyring::v1::Entry::new("com.softphone.desktop", account).map_err(|_| ())
    }

    pub fn get(account: &str) -> Result<String, ()> {
        entry(account)?.get_password().map_err(|_| ())
    }

    pub fn set(account: &str, value: &str) -> Result<(), ()> {
        entry(account)?.set_password(value).map_err(|_| ())
    }

    pub fn remove(account: &str) -> Result<(), ()> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::v1::Error::NoEntry) => Ok(()),
            Err(_) => Err(()),
        }
    }
}

#[cfg(target_os = "android")]
mod platform {
    use jni::{
        JNIEnv, JavaVM,
        errors::Result as JniResult,
        objects::{JObject, JString},
    };

    fn with_application<T>(
        op: impl FnOnce(&mut JNIEnv, &JObject) -> JniResult<T>,
    ) -> Result<T, ()> {
        let context = ndk_context::android_context();
        // android-activity owns this VM and global Application reference for the process lifetime.
        let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(|_| ())?;
        let mut env = vm.attach_current_thread().map_err(|_| ())?;
        let application = unsafe { JObject::from_raw(context.context().cast()) };
        let result = op(&mut env, &application);
        if result.is_err() {
            let _ = env.exception_clear();
        }
        result.map_err(|_| ())
    }

    pub fn get(account: &str) -> Result<String, ()> {
        with_application(|env, application| {
            let name = env.new_string(account)?;
            let value: JString = env
                .call_method(
                    application,
                    "readCredential",
                    "(Ljava/lang/String;)Ljava/lang/String;",
                    &[(&name).into()],
                )?
                .l()?
                .into();
            Ok(env.get_string(&value)?.into())
        })
    }

    pub fn set(account: &str, value: &str) -> Result<(), ()> {
        with_application(|env, application| {
            let name = env.new_string(account)?;
            let secret = env.new_string(value)?;
            env.call_method(
                application,
                "saveCredential",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[(&name).into(), (&secret).into()],
            )?
            .v()
        })
    }

    pub fn remove(account: &str) -> Result<(), ()> {
        with_application(|env, application| {
            let name = env.new_string(account)?;
            env.call_method(
                application,
                "removeCredential",
                "(Ljava/lang/String;)V",
                &[(&name).into()],
            )?
            .v()
        })
    }
}

pub use platform::{get, remove, set};
