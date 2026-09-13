# Credits

GitHub's contributors list is built from commit authorship, so it only ever
shows the handful of people who have pushed code. Almost everything this
driver knows about the hardware came from somewhere else: from people who
plugged in a wheel none of the maintainers own, ran a diagnostic that told
them nothing useful, and reported the result anyway.

This file is where that work is recorded. It is ordered by what people did,
not by how much they did.

If you are listed here and would rather not be, or the description is wrong,
open an issue and it will be changed the same day.

## Wheels nobody here owns

Support for a wheel we cannot touch is written blind and confirmed by proxy.
Every one of these people ran tests on hardware in their own home, often
several rounds of them, and often getting nothing back for their trouble
until the last round.

- **[fenixadam](https://github.com/fenixadam)** - the G923 Xbox edition on
  Logitech's own SDK, from the first capture to the release. His USB
  captures decoded the SDK's stream on that wheel ([#81](../../issues/81)),
  his hardware runs confirmed the rotation-range and strength fixes
  ([#82](../../issues/82)), his kernel traces named two separate crashes and
  the load-order race behind one of them ([#90](../../issues/90)), and a
  full day of precise runs on the SDK route found four faults in a row
  that no machine here could have shown: a marker written under the wrong
  name, an SDK left believing a tiny rotation range, a proxy that re-entered
  the SDK, and an installer that kept old apps ([#91](../../issues/91)). He
  then compared the result against Windows on the same car. AC Rally
  ([#83](../../issues/83)) is his too.
- **[DaveDos95](https://github.com/DaveDos95)** - RaceRoom on an RS50 under
  SteamOS ([#92](../../issues/92)): read the game's own profile file to rule
  out the first diagnosis, then produced the proxy logs that proved the real
  fault, a DirectInput force aimed straight north, and confirmed the fix
  with a binary built for a system that cannot build its own. The same runs
  were the first live confirmation of the RaceRoom telemetry decoder.
- **[simonr2k4](https://github.com/simonr2k4)** - the entire G923 Xbox
  edition. Force feedback and TrueForce both work on that wheel because of
  his testing, and neither existed before it ([#27](../../issues/27)). He
  also found that the installer shipped a udev rule without the helper it
  calls, which left the wheel looking dead, and produced the Wine log that
  root-caused the 90 degree steering lock: the TrueForce SDK looking for a
  G HUB that does not exist under Proton.
- **[pokesl0w](https://github.com/pokesl0w)** - the first confirmed Steam
  Deck install, and the G923 Xbox edition's button and pedal layouts,
  captured button by button from his own wheel ([#68](../../issues/68)).
  Both of those layouts differ from the PlayStation edition's, which is
  something no amount of reading could have established here.
- **[Maanikko81](https://github.com/Maanikko81)** - found that the G923
  reports its pedals inverted, with the evtest reading that made it
  provable rather than anecdotal ([#67](../../issues/67)).
- **[adnanmur](https://github.com/adnanmur)** - reported the G923 Xbox
  mode-switch rule hard enough to stop a machine booting, with a diagnosis
  better than the bug deserved ([#52](../../issues/52)).
- **[sixtysecondstosmash](https://github.com/sixtysecondstosmash)** and
  **[gmlinux](https://github.com/gmlinux)** - G PRO Racing Wheel captures
  ([#8](../../issues/8)), which is how that wheel's protocol was mapped.
- **[SandSeppel](https://github.com/SandSeppel)** - the earliest reports on
  the project, including the RS50 TrueForce capture
  ([#5](../../issues/5)) that the haptic work is built on, plus the first
  build failures ([#1](../../issues/1), [#2](../../issues/2)), the
  reconnection bug ([#6](../../issues/6)), and the original request for
  TrueForce streaming ([#4](../../issues/4)).

## Protocol analysis

- **[Mhytee](https://github.com/Mhytee)** - author of
  [TF4ALL](https://github.com/Mhytee/Trueforce-For-All), whose independent
  analysis confirmed the G923 shares the RS50 and G PRO TrueForce stream
  protocol, and corrected our understanding of how Windows drives these
  wheels ([#20](../../issues/20)).
- **[PeposCJ](https://github.com/PeposCJ)** - contributed to the same
  protocol discussion.

## Sustained bug reporting

- **[Ismael-Torresan](https://github.com/Ismael-Torresan)** - the only
  quantitative measurements the force engine has: a fixed push over a fixed
  travel, timed, for damper, friction and inertia on both the firmware's
  path and the engine's, twice ([#72](../../issues/72),
  [#87](../../issues/87)). Those numbers are what the engine's condition
  forces will be calibrated against, and his wedge report on the Xbox
  edition found a detector that reset itself on every error reply.
- **[Lorenzoo78](https://github.com/Lorenzoo78)** - the Assetto Corsa EVO
  stutter report under the launcher ([#74](../../issues/74)).
- **[matthiasvegh](https://github.com/matthiasvegh)** - twelve issues and
  the most thorough reporting the project has had: rotation range having no
  effect ([#10](../../issues/10)), inverted force feedback
  ([#12](../../issues/12)), cumulative forces ([#16](../../issues/16)),
  intermittent FFB spikes ([#31](../../issues/31)), rev-light brightness
  reverting ([#29](../../issues/29)), slow module init
  ([#30](../../issues/30)), and packaging assumptions that only break
  outside Arch ([#17](../../issues/17), [#18](../../issues/18)).
- **[gondezee](https://github.com/gondezee)** - the SDK revision question
  ([#21](../../issues/21)), a hat-switch direction bug
  ([#22](../../issues/22)), and Arch-specific test notes
  ([#23](../../issues/23)).
- **[andrewexton373](https://github.com/andrewexton373)** - kernel 7.0.9
  build breakage ([#24](../../issues/24)) and the Le Mans Ultimate pedal
  binding problem through the logi-ffb virtual wheel
  ([#50](../../issues/50)).

## Kernels and distributions

Regressions that turned out not to be ours, established by people who did
the work to prove it.

- **[Mesche900](https://github.com/Mesche900)** - held everything else
  constant across three kernels to show that a force feedback regression was
  Debian's 6.12 stable branch rather than this driver, and reported the
  result that made an earlier observation of their own wrong
  ([#53](../../issues/53)).
- **[AX3Lino](https://github.com/AX3Lino)** and
  **[LuanVSO](https://github.com/LuanVSO)** - reported that the driver
  interfered with Logitech mice ([#7](../../issues/7),
  [#9](../../issues/9)). That is why the module is scoped to wheels and no
  longer shadows the in-tree driver for every other Logitech device.
- **[marvicdigital](https://github.com/marvicdigital)** and
  **[sugituber](https://github.com/sugituber)** - additional reports and
  confirmations.

## Code

Commit authors appear in
[the contributors graph](../../graphs/contributors), including
**[aderumier](https://github.com/aderumier)**, whose pull request is in the
tree.

Upstream projects this driver is built on are credited in the
[README](README.md#acknowledgments).
