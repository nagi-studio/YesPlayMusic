# Tagged audio fixtures

这两个 Base64 fixture 是独立使用 FFmpeg 8.1 生成的短静音 MP3/FLAC，标签均为：

- title：`迁移曲目`
- album：`测试专辑`
- artist：`云音乐艺术家`

FLAC 另用 metaflac 1.5.0 删除了 padding；运行测试不依赖 FFmpeg 或 metaflac。解码后的 SHA-256：

- MP3：`cead90ef7848cca8113262289a69cec8f1f3ea08ab33aae911dbe75cce4cce1d`
- FLAC：`325b8641d42cf8ea4bf160da3eccbb8a4a26bc46ac22c874d8bade4e30130eeb`

fixture 由独立工具生成，避免使用被测的 `lofty` writer 自证 reader。
