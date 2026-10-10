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

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
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
    static final int SAVE_DOCUMENT = 4;
    static final int OPEN_DOCUMENT = 5;
    static final String CHANNEL_SYNC = "sync";
    private static final String CHANNEL_PROBLEMS = "problems";
    private static final String KEY_ALIAS = "celeste-secrets";
    private static final int IV_LENGTH = 12;

    /** A long pass ends with this, should the engine never say it ended. */
    private static final long WAKE_LOCK_TIMEOUT_MS = 60 * 60 * 1000;

    private static WeakReference<Activity> activity = new WeakReference<>(null);
    /** What {@link #saveDocument} writes once the user chose where. */
    private static byte[] pendingSave;
    private static PowerManager.WakeLock wakeLock;

    private Bridge() {}

    /** Answers {@link #saveDocument} and {@link #openDocument}: `error` when something went wrong, else `data` null when cancelled, the file's bytes after opening, empty after saving. */
    static native void nativeDocumentDone(byte[] data, String error);

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

    /** The one-line sync status: starts the sync service with it, or updates the service's notification. `sinceMillis` (0 for none) is shown as the status's age. */
    public static void showSyncStatus(Context context, String text, long sinceMillis) {
        SyncService.show(context, text, sinceMillis);
    }

    /** Starts the sync service, or stops it, after "Run in background" was switched. */
    public static void runInBackground(Context context, boolean on) {
        if (on) {
            SyncService.start(context);
        } else {
            context.stopService(new Intent(context, SyncService.class));
        }
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

    /** Whether Android leaves Celeste out of battery optimization, so it is neither frozen nor cut off from the network in the background. */
    public static boolean ignoresBatteryOptimizations(Context context) {
        return context.getSystemService(PowerManager.class).isIgnoringBatteryOptimizations(context.getPackageName());
    }

    /** Asks whether Celeste may always run in the background; where that dialog is missing, opens the list of optimized apps. */
    public static void askToIgnoreBatteryOptimizations(Context context) {
        try {
            context.startActivity(new Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:" + context.getPackageName())).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
        } catch (ActivityNotFoundException e) {
            try {
                context.startActivity(new Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            } catch (ActivityNotFoundException e2) {
                Log.w(TAG, "no battery optimization settings", e2);
            }
        }
    }

    /** Shows a folder in shared storage in the file manager: its document in the external storage provider, the reverse of {@link #treeToPath}. */
    public static void openFolder(Context context, String path) {
        String primary = Environment.getExternalStorageDirectory().getPath();
        String id;
        if (path.equals(primary) || path.startsWith(primary + "/")) {
            id = "primary:" + path.substring(Math.min(path.length(), primary.length() + 1));
        } else if (path.startsWith("/storage/")) {
            String rest = path.substring("/storage/".length());
            int slash = rest.indexOf('/');
            id = slash < 0 ? rest + ":" : rest.substring(0, slash) + ":" + rest.substring(slash + 1);
        } else {
            Log.w(TAG, "not in shared storage: " + path);
            return;
        }
        Uri folder = DocumentsContract.buildDocumentUri("com.android.externalstorage.documents", id);
        Intent intent = new Intent(Intent.ACTION_VIEW).setDataAndType(folder, DocumentsContract.Document.MIME_TYPE_DIR).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            context.startActivity(intent);
        } catch (ActivityNotFoundException | SecurityException e) {
            Log.w(TAG, "no app shows " + folder, e);
        }
    }

    // Saving and opening a file ---------------------------------------------

    /** Asks where to save `data`, suggesting `name`; the answer goes to {@link #nativeDocumentDone}. False when no activity is there to show the dialog. */
    public static boolean saveDocument(String name, byte[] data) {
        Activity current = activity.get();
        if (current == null) {
            return false;
        }
        pendingSave = data;
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("application/json").putExtra(Intent.EXTRA_TITLE, name);
        startForDocument(current, intent, SAVE_DOCUMENT);
        return true;
    }

    /** Asks for a file to read; the answer goes to {@link #nativeDocumentDone}. False when no activity is there to show the dialog. */
    public static boolean openDocument() {
        Activity current = activity.get();
        if (current == null) {
            return false;
        }
        // Any type: file managers and cloud apps often don't label JSON as such.
        startForDocument(current, new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*"), OPEN_DOCUMENT);
        return true;
    }

    private static void startForDocument(Activity current, Intent intent, int requestCode) {
        current.runOnUiThread(() -> {
            try {
                current.startActivityForResult(intent, requestCode);
            } catch (ActivityNotFoundException e) {
                nativeDocumentDone(null, "No app on this device saves or opens files.");
            }
        });
    }

    /** The result of {@link #saveDocument} or {@link #openDocument}, from the activity. */
    static void documentChosen(Context context, int requestCode, Uri uri) {
        byte[] data = pendingSave;
        pendingSave = null;
        if (uri == null) {
            nativeDocumentDone(null, null);
            return;
        }
        // Off the UI thread: the file may live with a cloud app that takes its time.
        new Thread(() -> {
            try {
                if (requestCode == SAVE_DOCUMENT) {
                    try (OutputStream out = context.getContentResolver().openOutputStream(uri, "wt")) {
                        out.write(data);
                    }
                    nativeDocumentDone(new byte[0], null);
                } else {
                    try (InputStream in = context.getContentResolver().openInputStream(uri)) {
                        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
                        byte[] buffer = new byte[8192];
                        for (int n; (n = in.read(buffer)) > 0; ) {
                            bytes.write(buffer, 0, n);
                        }
                        nativeDocumentDone(bytes.toByteArray(), null);
                    }
                }
            } catch (Exception e) {
                Log.w(TAG, "could not use " + uri, e);
                nativeDocumentDone(null, String.valueOf(e.getMessage()));
            }
        }).start();
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
