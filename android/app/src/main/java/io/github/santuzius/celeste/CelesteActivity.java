package io.github.santuzius.celeste;

import android.Manifest;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;

import io.github.santuzius.icedandroid.IcedActivity;

/** Celeste's window. The GUI itself is iced, drawn by the native library; this adds what only an activity can do: ask for permissions and show the folder picker. */
public class CelesteActivity extends IcedActivity {
    private static final int ASK_NOTIFICATIONS = 3;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Bridge.setActivity(this);

        // Without it the sync status and sign-in requests stay hidden from Android 13 on; syncing works either way.
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] { Manifest.permission.POST_NOTIFICATIONS }, ASK_NOTIFICATIONS);
        }
    }

    @Override
    protected void onDestroy() {
        Bridge.clearActivity(this);
        super.onDestroy();
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == Bridge.PICK_FOLDER) {
            Bridge.nativeFolderPicked(resultCode == RESULT_OK && data != null ? Bridge.treeToPath(data.getData()) : null);
            return;
        }
        super.onActivityResult(requestCode, resultCode, data);
    }
}
