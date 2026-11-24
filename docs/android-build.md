# Android APK build guide

This guide covers the mobile packaging toolchain for the ARM64 Android distribution. The seven system views describe the desktop process; this page explains how to rebuild `dist/wise-softphone-0.2.0-arm64-v8a.apk` from source. Linux is the tested platform. The commands below target Debian/Ubuntu.

## What the build needs

| Component | Version / package |
| --- | --- |
| JDK | 17 (used by Gradle and for signing keys) |
| Android SDK | `platform-tools`, `platforms;android-35`, `build-tools;35.0.1` |
| Android NDK | r28c, i.e. `ndk;28.2.13676358` (must match `ndkVersion` in `android/app/build.gradle`) |
| Rust | Rust 1.95 or newer with the `aarch64-linux-android` target |
| Environment | `JAVA_HOME`, `ANDROID_HOME`, `ANDROID_NDK_HOME` |

The runtime APK targets Android 9 or later, arm64-v8a only.

## 1. Install JDK 17

```sh
sudo apt update
sudo apt install -y openjdk-17-jdk
```

## 2. Install the Android SDK command-line tools

The command-line tools live in the user's home directory, so no root access is needed:

```sh
mkdir -p ~/Android/Sdk/cmdline-tools
cd /tmp
wget https://dl.google.com/android/repository/commandlinetools-linux-13114758_latest.zip
unzip -q commandlinetools-linux-13114758_latest.zip -d ~/Android/Sdk/cmdline-tools
mv ~/Android/Sdk/cmdline-tools/cmdline-tools ~/Android/Sdk/cmdline-tools/latest
```

`latest` is the directory name `sdkmanager` expects inside `cmdline-tools`.

## 3. Install the SDK packages

```sh
export ANDROID_HOME=$HOME/Android/Sdk
export PATH="$ANDROID_HOME/cmdline-tools/latest/bin:$PATH"
yes | sdkmanager --licenses
sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.1" "ndk;28.2.13676358"
```

This step downloads about 2.5 GB. NDK releases 28.x map to the four-part version numbers used by `sdkmanager`; only `28.2.13676358` (r28c) matches the build.

## 4. Set the environment variables

Append them to `~/.bashrc` so every shell has them:

```sh
cat >> ~/.bashrc <<'EOF'

# Android / Java toolchain
export JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64
export ANDROID_HOME=$HOME/Android/Sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/28.2.13676358
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$PATH"
EOF
source ~/.bashrc
```

`ANDROID_NDK_HOME` is not optional: Gradle reads `ndkPath` from it, and `build-native.sh` aborts without it.

## 5. Build the APK

From the repository root:

```sh
rustup target add aarch64-linux-android
android/package-apk.sh
```

The script prints the absolute path of the signed, verified APK when it finishes:

```
dist/wise-softphone-0.2.0-arm64-v8a.apk
```

Open it on the phone and allow microphone access for calls. Keep the app open to receive incoming calls; Android may suspend the SIP connection in the background.

## What `package-apk.sh` does

1. Requires `JAVA_HOME` and `ANDROID_HOME`, and runs `gradlew :app:assembleRelease` in `android/`. The first run downloads Gradle 8.11.1 (~130 MB).
2. The Gradle build calls `android/build-native.sh`, which cross-compiles the SIP/audio library with the NDK clang toolchain: `cargo build --locked --release --target aarch64-linux-android`, then copies `libsoftphone.so` into the `arm64-v8a` JNI directory.
3. On the first successful build it creates a signing identity in the private, Git-ignored `.android-signing/` directory (mode 700): `wise-softphone.p12` and a random `store.pass` file.
4. `zipalign` aligns the unsigned APK, `apksigner` signs it with the PKCS12 keystore, and the script verifies both the signature and the alignment.

### Signing identity

Back up `.android-signing/` securely. Rebuilding without it creates a new key, and Android requires the same signing identity to update an installed app. Set `WISE_SIGNING_DIR` to keep the directory somewhere else. The keystore files must never be included in an APK.

Other overrides: `GRADLE_BIN` selects an alternative `gradlew` executable. The version number in the output file name is hard-coded in the script.

## Troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| `Set JAVA_HOME to JDK 17` | No JDK on the machine or `JAVA_HOME` unset. Install JDK 17 and export the variable (steps 1 and 4). |
| `sdkmanager` warns the JDK is old or refuses to start | Recent command-line tool versions expect a newer JDK than 17. A warning alone is harmless. If it refuses, run `sdkmanager` with a newer JDK on `PATH` while keeping `JAVA_HOME` at 17 for Gradle, or install an older cmdline-tools archive. |
| `Set ANDROID_NDK_HOME to Android NDK r28c` | `ANDROID_NDK_HOME` unset or pointing at the wrong version. It must be the `ndk/28.2.13676358` directory (step 4). |
| Gradle fails with NDK not found | `ndkVersion '28.2.13676358'` in `android/app/build.gradle` reads `ndkPath` from `ANDROID_NDK_HOME`; install that exact NDK with `sdkmanager`. |
| `zipalign` or `apksigner` not found | `ANDROID_HOME/build-tools/35.0.1` missing; install `build-tools;35.0.1` (step 3). |
| License errors from `sdkmanager` | Run `yes \| sdkmanager --licenses` before installing packages. |
| Build works, phone rejects the update | The signing identity changed. Restore the original `.android-signing/` backup and rebuild. |

See `android/package-apk.sh`, `android/build-native.sh`, and `android/app/build.gradle` for the exact commands.
