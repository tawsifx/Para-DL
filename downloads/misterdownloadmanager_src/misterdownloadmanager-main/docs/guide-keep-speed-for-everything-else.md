# Tutorial: keep some speed for everything else

[← Back to docs index](README.md)

A download manager can fill your whole connection. These steps cap MDM so that browsing, video calls and games stay smooth.

## Cap MDM as a whole (recommended)

1. Choose **Downloads → Speed Limiter → Settings...**.
2. Tick **Limit all downloads together to** and type a speed in KBytes/sec. A good start is about two thirds of your connection's speed. (1 MByte/sec is about 1000 KBytes/sec.)
3. Tick **Only while I'm using the computer** if you want full speed when you're away. MDM goes back to full speed after a minute without mouse or keyboard input.
4. Click **OK**.

This cap is shared by every download, however many run at once. It works on its own; you don't need to turn the Speed Limiter on.

To remove it, open the same window and untick **Limit all downloads together to**.

![Speed Limiter settings: the maximum speed for all downloads](images/help/speed-limiter.webp)
*Downloads → Speed Limiter → Settings...: one speed for all of MDM.*

## Cap each file instead

1. In the same window, set **Maximum download speed for one file**. It starts at 10 KBytes/sec, which is very slow, so set it first.
2. Click **OK**.
3. Choose **Downloads → Speed Limiter → Turn on**.

Each download is then held to that speed. **Downloads → Speed Limiter → Turn off** gives full speed again. Tick **Always turn on Speed Limiter on Mister Download Manager startup** to have it on every time.

## Slow down one download only

1. Double-click the download to open its progress window.
2. Open the **Speed Limiter** tab.
3. Tick **Use Speed Limiter** and type the speed in **Maximum download speed**.

The other downloads keep their speed.

> Some servers drop the connection when the speed is very low. Don't set a tight limit for a server that can't resume (**Resume capability: No** in the progress window).

## Next steps

- [Speed Limiter and connections in detail →](connections-and-speed.md#speed-limiter)
- [Download overnight →](guide-download-overnight.md)
- [Download limits for capped internet plans →](queues-and-scheduler.md#download-limits)
