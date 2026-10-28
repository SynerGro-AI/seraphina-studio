# Camera fixtures

Recorded camera frames for the camera tests in `src/bin/ui/camera.rs` and
`src/bin/ui/output.rs`. A test reader plays them through the same decoder
the V4L2 capture path uses, so the tests run with no camera attached.

* `red_320x180.mjpeg`: 5 MJPEG frames, solid red (200, 40, 40), 320x180, the
  format a USB webcam sends.

      ffmpeg -f lavfi -i color=c=0xC82828:s=320x180:r=30 -frames:v 5 \
        -pix_fmt yuvj422p -c:v mjpeg -f mjpeg red_320x180.mjpeg

* `testsrc_64x48.yuyv`: 10 raw packed YUYV 4:2:2 frames of ffmpeg's test
  pattern, 64x48, no padding (6144 bytes per frame).

      ffmpeg -f lavfi -i testsrc=s=64x48:r=30 -frames:v 10 \
        -pix_fmt yuyv422 -f rawvideo testsrc_64x48.yuyv

Made with ffmpeg 7.1.
