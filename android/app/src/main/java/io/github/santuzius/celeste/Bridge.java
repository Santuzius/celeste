package io.github.santuzius.celeste;

import android.Manifest;
import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.os.Environment;
import android.os.PowerManager;
import android.provider.DocumentsContract;
import android.provider.Settings;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Log;

import java.lang.ref.WeakReference;
import java.security.KeyStore;
import java.util.Arrays;

import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/**
 * Static helpers the native library calls (src/infrastructure/android/ in the repository): notifications, the sync service, the folder picker and the Keystore.
 *
 * They run on whatever native thread calls them; UI work is posted to the activity's UI thread.
 */
public final class Bridge {
    static final String TAG = "celeste";
    static final int PICK_FOLDER = 1;
    private static final int ASK_STORAGE = 2;
    static final String CHANNEL_SYNC = "sync";
    private static final String CHANNEL_PROBLEMS = "problems";
    private static final String KEY_ALIAS = "celeste-secrets";
    private static final int IV_LENGTH = 12;

    /** A long pass ends with this, should the engine never say it ended. */
    private static final long WAKE_LOCK_TIMEOUT_MS = 60 * 60 * 1000;

    private static WeakReference<Activity> activity = new WeakReference<>(null);
    private static PowerManager.WakeLock wakeLock;

    private Bridge() {}

    /** The picked folder's path, or null when cancelled or when it has none. Answers {@link #pickFolder}. */
    static native void nativeFolderPicked(String path);

    static void setActivity(Activity current) {
        activity = new WeakReference<>(current);
    }

    static void clearActivity(Activity destroyed) {
        if (activity.get() == destroyed) {
            activity = new WeakReference<>(null);
        }
    }

    // Notifications ---------------------------------------------------------

    static void createChannels(Context context) {
        NotificationManager manager = context.getSystemService(NotificationManager.class);
        // Minimal importance: the status sits silently in the shade without an icon in the status bar.
        manager.createNotificationChannel(new NotificationChannel(CHANNEL_SYNC, "Sync status", NotificationManager.IMPORTANCE_MIN));
        manager.createNotificationChannel(new NotificationChannel(CHANNEL_PROBLEMS, "Problems", NotificationManager.IMPORTANCE_DEFAULT));
    }

    /** Opens Celeste, or brings it back to the front. */
    static PendingIntent openApp(Context context) {
        Intent intent = new Intent(context, CelesteActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        return PendingIntent.getActivity(context, 0, intent, PendingIntent.FLAG_IMMUTABLE);
    }

    /** A notification about something the user has to do, e.g. sign in again. The same text replaces its earlier notification. */
    public static void notify(Context context, String title, String text) {
        createChannels(context);
        Notification notification = new Notification.Builder(context, CHANNEL_PROBLEMS)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle(title)
                .setContentText(text)
                .setStyle(new Notification.BigTextStyle().bigText(text))
                .setContentIntent(openApp(context))
                .setAutoCancel(true)
                .build();
        context.getSystemService(NotificationManager.class).notify(text.hashCode(), notification);
    }

    /** The one-line sync status: starts the sync service with it, or updates the service's notification. */
    public static void showSyncStatus(Context context, String text) {
        SyncService.show(context, text);
    }

    /** Keeps the CPU running while a sync pass runs, so it finishes with the screen off; Android would otherwise suspend it halfway. */
    public static synchronized void keepAwake(Context context, boolean awake) {
        if (wakeLock == null) {
            wakeLock = context.getSystemService(PowerManager.class).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "celeste:sync");
            wakeLock.setReferenceCounted(false);
        }
        if (awake) {
            wakeLock.acquire(WAKE_LOCK_TIMEOUT_MS);
        } else if (wakeLock.isHeld()) {
            wakeLock.release();
        }
    }

    /** Opens a link in the default browser. */
    public static void openUrl(Context context, String url) {
        try {
            context.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        } catch (ActivityNotFoundException e) {
            Log.w(TAG, "no app opens " + url, e);
        }
    }

    // Folder picker ---------------------------------------------------------

    /** Opens the system's folder picker; the answer goes to {@link #nativeFolderPicked}. Without access to shared storage it opens that setting instead and answers null. False when no activity is there to show it. */
    public static boolean pickFolder() {
        Activity current = activity.get();
        if (current == null) {
            return false;
        }
        current.runOnUiThread(() -> {
            if (!hasStorageAccess(current)) {
                askForStorageAccess(current);
                nativeFolderPicked(null);
                return;
            }
            try {
                current.startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), PICK_FOLDER);
            } catch (ActivityNotFoundException e) {
                nativeFolderPicked(null);
            }
        });
        return true;
    }

    /** Whether Celeste may read and write anywhere in shared storage, which syncing folders there needs. */
    static boolean hasStorageAccess(Context context) {
        if (Build.VERSION.SDK_INT >= 30) {
            return Environment.isExternalStorageManager();
        }
        return context.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED;
    }

    private static void askForStorageAccess(Activity current) {
        if (Build.VERSION.SDK_INT < 30) {
            current.requestPermissions(new String[] { Manifest.permission.WRITE_EXTERNAL_STORAGE }, ASK_STORAGE);
            return;
        }
        try {
            current.startActivity(new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:" + current.getPackageName())));
        } catch (ActivityNotFoundException e) {
            current.startActivity(new Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION));
        }
    }

    /** The file path of a folder from the picker: "primary:Documents/Notes" is /storage/emulated/0/Documents/Notes, other volumes are under /storage/<id>. Null for folders of other providers (cloud apps, Downloads), which have no path. */
    static String treeToPath(Uri tree) {
        if (tree == null || !"com.android.externalstorage.documents".equals(tree.getAuthority())) {
            return null;
        }
        String id = DocumentsContract.getTreeDocumentId(tree);
        int colon = id.indexOf(':');
        if (colon < 0) {
            return null;
        }
        String volume = id.substring(0, colon);
        String relative = id.substring(colon + 1);
        String root = "primary".equals(volume) ? Environment.getExternalStorageDirectory().getPath() : "/storage/" + volume;
        return relative.isEmpty() ? root : root + "/" + relative;
    }

    // Keystore --------------------------------------------------------------

    /** AES-GCM with a key that never leaves the Android Keystore. Returns the IV followed by the ciphertext, null on failure. */
    public static synchronized byte[] encrypt(byte[] plain) {
        try {
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.ENCRYPT_MODE, key());
            byte[] iv = cipher.getIV();
            byte[] sealed = cipher.doFinal(plain);
            byte[] out = Arrays.copyOf(iv, iv.length + sealed.length);
            System.arraycopy(sealed, 0, out, iv.length, sealed.length);
            return out;
        } catch (Exception e) {
            Log.e(TAG, "encrypt failed", e);
            return null;
        }
    }

    /** Reverses {@link #encrypt}; null on failure (wrong key, tampered data). */
    public static synchronized byte[] decrypt(byte[] sealed) {
        try {
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, sealed, 0, IV_LENGTH));
            return cipher.doFinal(sealed, IV_LENGTH, sealed.length - IV_LENGTH);
        } catch (Exception e) {
            Log.e(TAG, "decrypt failed", e);
            return null;
        }
    }

    private static SecretKey key() throws Exception {
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        if (store.containsAlias(KEY_ALIAS)) {
            return ((KeyStore.SecretKeyEntry) store.getEntry(KEY_ALIAS, null)).getSecretKey();
        }
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        generator.init(new KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build());
        return generator.generateKey();
    }
}
