package com.wise.softphone;

import android.Manifest;
import android.app.AlertDialog;
import android.content.pm.PackageManager;
import android.os.Bundle;

import com.google.androidgamesdk.GameActivity;


public final class WiseActivity extends GameActivity {
    private static final int MICROPHONE = 1;

    static { System.loadLibrary("softphone"); }

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        if (state == null && checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] {Manifest.permission.RECORD_AUDIO}, MICROPHONE);
        }
    }

    @Override public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(request, permissions, results);
        if (request == MICROPHONE && results.length > 0 && results[0] != PackageManager.PERMISSION_GRANTED) {
            new AlertDialog.Builder(this).setTitle("Microphone access")
                .setMessage("To send your voice on calls, allow microphone access in Android app settings.")
                .setPositiveButton(android.R.string.ok, null).show();
        }
    }

}
