package com.wise.softphone;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import android.util.Base64;

public final class WiseApplication extends Application {
    private static final String KEY_ALIAS = "wise.softphone.sip";

    // Called by the Rust credential worker. Only ciphertext and the IV go into app storage.
    private SharedPreferences credentials() { return getSharedPreferences("sip", Context.MODE_PRIVATE); }

    private SecretKey key(boolean create) throws Exception {
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        if (store.containsAlias(KEY_ALIAS)) { return (SecretKey) store.getKey(KEY_ALIAS, null); }
        if (!create) { throw new IllegalStateException("Saved key is unavailable"); }
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        generator.init(new KeyGenParameterSpec.Builder(KEY_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build());
        return generator.generateKey();
    }

    public void saveCredential(String account, String value) throws Exception {
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.ENCRYPT_MODE, key(true));
        byte[] encrypted = cipher.doFinal(value.getBytes(StandardCharsets.UTF_8));
        String stored = Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP) + "." +
            Base64.encodeToString(encrypted, Base64.NO_WRAP);
        if (!credentials().edit().putString(account, stored).commit()) {
            throw new IllegalStateException("Saving the credential failed");
        }
    }

    public String readCredential(String account) throws Exception {
        String stored = credentials().getString(account, null);
        if (stored == null) { throw new IllegalStateException("Saved password is unavailable"); }
        String[] parts = stored.split("\\.", 2);
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.DECRYPT_MODE, key(false), new GCMParameterSpec(128, Base64.decode(parts[0], Base64.NO_WRAP)));
        return new String(cipher.doFinal(Base64.decode(parts[1], Base64.NO_WRAP)), StandardCharsets.UTF_8);
    }

    public void removeCredential(String account) {
        if (!credentials().edit().remove(account).commit()) {
            throw new IllegalStateException("Removing the credential failed");
        }
    }
}
