package ai.storyteller.effectcraft;

import com.google.androidgamesdk.GameActivity;

import android.content.Intent;
import android.os.Bundle;

public class MainActivity extends GameActivity {

    static {
        // Loads the Rust cdylib (the AndroidManifest's android.app.lib_name=main).
        System.loadLibrary("main");
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == SafBridge.REQUEST_CODE || requestCode == SafBridge.REQUEST_SAVE) {
            SafBridge.onResult(this, requestCode, resultCode == RESULT_OK ? data : null);
        }
    }

    /**
     * Rust calls these instance methods: calling a method on an object we already hold needs no
     * class lookup, which a natively attached thread cannot do for application classes (the JNI
     * spec gives it the system class loader). Bulk data travels through a manifest file, not JNI.
     */
    public void safPickMedia() {
        SafBridge.pickMedia(this);
    }

    /** Whether the import picker and the copy are finished. */
    public boolean safIsDone() {
        return SafBridge.isDone();
    }

    /** Open the "where should this be saved?" picker. */
    public void safPickSave(String title) {
        SafBridge.pickSave(this, title);
    }

    public boolean safSaveDone() {
        return SafBridge.isSaveDone();
    }

    public boolean safHasSaveUri() {
        return SafBridge.hasSaveUri();
    }

    /** Copy a finished file to the chosen destination. */
    public boolean safPublish(String path) {
        return SafBridge.publish(this, path);
    }

    /** Open an https link (Help / About buttons) in the browser. */
    public boolean safOpenUrl(String url) {
        return SafBridge.openUrl(this, url);
    }
}
