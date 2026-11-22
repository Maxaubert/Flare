package dev.flare.probe;

import android.app.Activity;
import android.os.Bundle;
import android.text.Editable;
import android.text.TextWatcher;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

/** A text field for the agent helper's set-text (rule 1368): an EditText with a hint, and a line below that repeats
 *  what the field holds, so a check can read it from the tree too. Start it with
 *  `am start -n dev.flare.probe/.TextProbe`. */
public class TextProbe extends Activity {
    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        LinearLayout l = new LinearLayout(this);
        l.setOrientation(LinearLayout.VERTICAL);
        l.setPadding(48, 160, 48, 48);
        final EditText field = new EditText(this);
        field.setHint("Type here");
        field.setContentDescription("probe field");
        final TextView echo = new TextView(this);
        echo.setContentDescription("probe echo");
        field.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int a, int c, int d) {}
            @Override public void onTextChanged(CharSequence s, int a, int c, int d) {}
            @Override public void afterTextChanged(Editable s) {
                echo.setText("echo:" + s);
            }
        });
        l.addView(field);
        l.addView(echo);
        setContentView(l);
        field.requestFocus();
    }
}
