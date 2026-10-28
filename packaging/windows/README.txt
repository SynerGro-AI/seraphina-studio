Seraphina Studio (Windows x64)

Double-click SeraphinaStudio.exe. Keep the assets folder and out.* files next to it.
You can also open a file directly:  SeraphinaStudio.exe path\to\image.psd
Built with the x86_64-pc-windows-gnu toolchain. Tested under Wine 10 on Linux
(a proxy, not real Windows) and built + unit-tested by CI on windows-latest.

RECORDING AND STREAMING NEED FFMPEG (not bundled)
-------------------------------------------------
Record (MP4: H.264 + AAC) and Go live (RTMP) run ffmpeg as a subprocess.
ffmpeg is not included because a static GPL build (needed for libx264) is
about 170 MB. To enable it:
  1. Download a Windows build, e.g.
       https://www.gyan.dev/ffmpeg/builds/          (ffmpeg-release-essentials.zip)
       https://github.com/BtbN/FFmpeg-Builds/releases  (ffmpeg-master-latest-win64-gpl.zip)
  2. Copy bin\ffmpeg.exe (and ffprobe.exe if you like) next to SeraphinaStudio.exe,
     or add its folder to PATH, or set SERAPHINA_FFMPEG=C:\full\path\ffmpeg.exe
  3. Restart Seraphina Studio. Stream mode > Output shows "ffmpeg found".
Lookup order: SERAPHINA_FFMPEG, next to the exe (or .\ffmpeg\bin\), then PATH.
Without ffmpeg, Record / Go live show a clear "ffmpeg was not found" message.

ffmpeg licence: those builds are GPL v3 (they include libx264). If you
redistribute Seraphina Studio together with ffmpeg.exe you must ship the
ffmpeg LICENSE and offer its source (https://ffmpeg.org/download.html).
Seraphina Studio only runs ffmpeg as a separate program; it does not link it.

Output settings (Settings > Output): RTMP URL, stream key, size, fps,
bitrate, audio (none / silent AAC / DirectShow device), record folder,
reconnect delay and attempts. Settings are saved in
%USERPROFILE%\.seraphina\studio\output.json; the stream key is stored on its
own in %USERPROFILE%\.seraphina\studio\stream_key, shown masked, and never
sent to the AI assistant or written to logs. Recordings default to
%USERPROFILE%\.seraphina\studio\recordings.

AI (OPTIONAL)
-------------
The Seraphina assistant and Grok Imagine need an xAI API key
(https://console.x.ai). Without one the assistant runs offline and says so;
nothing is faked.

Add the key in the app: Settings (gear icon, top right) > Seraphina > xAI API
key, paste it, then Save (Test key checks it first). It is stored in Windows
Credential Manager as "SeraphinaStudio/xai_api_key" (per user, not roaming),
shown masked, and never written to logs, crash reports or project files.
Remove it there with Forget.

Privacy: the first time you send a chat message to xAI, Seraphina asks
first and lists what goes with it: the conversation, app state (mode, layer
and object names, panel layout) and recent Seraphina.AGI memory notes.
Choose Allow and send, Send only my messages, or Don't send. Change it any
time in Settings > Privacy. Pixels, files and your keys are not sent with
chat. Keys are only ever sent over https (plain http only to localhost).

Cost: xAI bills your key per chat message, per image and per second of
video. The status bar (bottom right) shows what this session spent; Settings > xAI
spending shows the session, this project and the last 24 hours, and lets
you set a spend cap (USD per 24 h, 0 = off). Once the cap is reached
nothing more is sent to xAI and a message says so. Costs use xAI's own
figures when it reports them, otherwise estimates from the public price
list (video is always estimated). Stop (shown while Seraphina or Grok
Imagine is working) ends the wait at once, but a request xAI had already
received may still be billed. Slow or failing requests time out and are
retried a few times with a growing delay.

Environment variables (optional; they override the saved key and defaults,
restart after changing):
  XAI_API_KEY             xAI key (or SERAPHINA_GROK_API_KEY)
  SERAPHINA_MODEL         chat model (default grok-4.7)
  SERAPHINA_BASE_URL      OpenAI-compatible endpoint (default https://api.x.ai/v1)
  SERAPHINA_VIDEO_MODEL   default grok-imagine-video-1.5 (billed per second)
  SERAPHINA_IMAGE_MODEL   default grok-imagine-image-2.0
  SERAPHINA_PRICE_IN / SERAPHINA_PRICE_OUT   chat price per million tokens, for the meter
The Seraphina.AGI CLI (remember / recall / triad) is used when `seraphina`
is on PATH (pip install seraphina-agi); those run on this PC.
Set SERAPHINA_NATIVE_FRAME=1 to use the normal Windows title bar.
