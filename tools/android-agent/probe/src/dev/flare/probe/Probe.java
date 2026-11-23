package dev.flare.probe;

import android.app.Activity;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.os.Bundle;
import android.view.MotionEvent;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;

/** Flare's probe app (tools/android-agent/probe, never shipped; Android plan PR A3), from Step 0 S1 and S2: paints a frame counter as a barcode every vsync, in two bands (top and bottom) so a torn
 *  frame shows as two different counters. A red 64x64 square marks the top-left corner (row order check). The
 *  middle band flips black/white on each touch down, at once. */
public class Probe extends Activity {
    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN | WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        setContentView(new Bars(this));
    }

    static class Bars extends View {
        final Paint p = new Paint();
        int frame = 0;
        boolean touched = false;
        Bars(Context c) { super(c); }

        @Override public boolean onTouchEvent(MotionEvent e) {
            if (e.getActionMasked() == MotionEvent.ACTION_DOWN) { touched = !touched; invalidate(); }
            return true;
        }

        void band(Canvas c, int top, int h, int w) {
            int bits = 16, bw = w / bits;
            for (int i = 0; i < bits; i++) {
                p.setColor(((frame >> (bits - 1 - i)) & 1) == 1 ? Color.WHITE : Color.BLACK);
                c.drawRect(i * bw, top, (i + 1) * bw, top + h, p);
            }
        }

        @Override protected void onDraw(Canvas c) {
            int w = getWidth(), h = getHeight();
            frame++;
            c.drawColor(Color.GRAY);
            band(c, h / 10, h / 10, w);
            p.setColor(touched ? Color.WHITE : Color.BLACK);
            c.drawRect(0, h * 4 / 10, w, h * 6 / 10, p);
            band(c, h * 8 / 10, h / 10, w);
            p.setColor(Color.RED);
            c.drawRect(300, 150, 364, 214, p);
            postInvalidateOnAnimation();
        }
    }
}

