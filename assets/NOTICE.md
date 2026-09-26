# Third-Party Assets & Attribution Notice

This repository contains data files, protocol mappings, and game assets necessary for Minecraft protocol compatibility across multiple versions.

---

### Protocol Version Translation (ViaVersion / ViaBackwards / ViaRewind)
* **Files**: `assets/viabackwards/`, `assets/viarewind/`
* **Copyright**: © ViaVersion contributors (https://github.com/ViaVersion).
* **License**: The translator code is GPL-3.0-only. Vendored mapping/data files retain the terms of their specific upstream source; this notice does not classify both directories under a blanket MIT license. ViaBackwards and ViaRewind are GPLv3 projects. ViaVersion/Mappings separately states that files in its `mappings/` directory may be copied, used, and expanded. Record the source and revision when updating vendored files.
* **Upstream projects**: [ViaVersion](https://github.com/ViaVersion/ViaVersion), [ViaBackwards](https://github.com/ViaVersion/ViaBackwards), [ViaRewind](https://github.com/ViaVersion/ViaRewind), and [Via Mappings](https://github.com/ViaVersion/Mappings).
* **1.21 enchantment definitions**: `assets/viaversion/data/enchantments-1.21.nbt` is copied from ViaVersion tag `5.12.0`, `common/src/main/resources/assets/viaversion/data/enchantments-1.21.nbt`; copyright ViaVersion contributors, GPL-3.0.
* **1.16 translation subset**: `assets/viabackwards/data/translations-1.16.json` is the `1.16` section extracted from ViaBackwards `translation-mappings.json` at revision `7bb4a5d69c56930f7543da48e32bb489d04c0cba`.
* **1.15.2–1.16.1 metadata ids**: `assets/meta_data_type/1_15_2_meta_data_type.json` follows ViaVersion `EntityDataTypes1_14.java` and `Types1_16.java` at revision `b4023127f6d1f8d6b0c1f1893e09fe1e2f8a72ac`; the upstream API source carries its MIT notice.
* **1.9–1.14 metadata ids**: `assets/meta_data_type/1_9_meta_data_type.json`, `1_12_meta_data_type.json`, `1_13_meta_data_type.json`, `1_13_2_meta_data_type.json`, and `1_14_meta_data_type.json` follow ViaVersion `EntityDataTypes1_9.java`, `EntityDataTypes1_12.java`, and `EntityDataTypes1_13`/`1_13_2`/`1_14.java` at revision `b4023127f6d1f8d6b0c1f1893e09fe1e2f8a72ac`; the upstream API source carries its MIT notice.
