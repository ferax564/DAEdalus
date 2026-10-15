# Examples

`Daedalus.Examples` rebuilds the systems from six public engineering blog posts
by Modelon and Claytex, using components from the bundled library.

The vendor models and parameter sets behind those posts are not public.
Each example therefore states:
- its source;
- what it reproduces;
- the parameter values that were assumed.

The aim is the same physics and the same kind of result plot, not identical
numbers. Every example has a regression test in
`crates/daedalus-core/tests/library.rs`, and CI simulates them all.

Run any one of them with:

```sh
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.<Package>.<Model> -o out.mat
```

Or open it in the workbench and press **Simulate**.

| Example | Source | States | Steps | CPU |
| --- | --- | ---: | ---: | ---: |
| TorqueConverter.StallTest | Claytex | 2 | 158 | 2 ms |
| TorqueConverter.Launch | Claytex | 9 | 644 | 13 ms |
| LandingGear.SinkRates | Modelon | 15 | 913 | 10 ms |
| AileronActuator.PositionControl | Modelon | 6 | 2,718 | 15 ms |
| AileronActuator.SpeedVsLoad | Modelon | 6 | 5,392 | 24 ms |
| QuarterCar.BumpComparison | Claytex | 22 | 1,563 | 28 ms |
| VehicleEfficiency.NEDCSensitivity | Claytex | 28 | 108,900 | 2.3 s |
| VehicleEfficiency.CoastDown | Modelon | 2 | 55 | 1 ms |
| VehicleEfficiency.UDDSEnergy | Modelon | 12 | 30,715 | 0.34 s |

Times are from `daedalus test-library library/Daedalus -m Daedalus` in a
release build, including translation.

## Mechanical and automotive

### Torque converter: stall test and launch

*Source: Claytex, "Torque converters: the clutches of automatic transmissions"
(claytex.com/tech-blog/torque-converters-the-clutches-of-automatic-transmissions/).*

**StallTest** holds the turbine and runs the engine at full throttle. The engine
settles where its full-load torque equals the impeller torque `(n/K)^2`.
- Simulated stall speed: **2160 rpm**.
- The K-factor prediction agrees to within 1 rpm.
- The turbine sees 2.1 times the engine torque.

**Launch** runs a full-throttle launch twice:
- once with a static K-factor converter;
- once with a 50 ms fluid torque build-up lag.

In both cases the engine flares toward stall speed and the torque ratio falls
to 1 at the coupling point. The lock-up clutch then closes and pulls the engine
down to turbine speed.

Assumed values:
- a generic three-element converter, K = 145 rpm/√(N·m);
- a 250 N·m engine;
- a 1700 kg car, first gear 4.2, final drive 3.2.

### Quarter car with a physical hydraulic damper

*Source: Claytex, "A look inside VeSyMA's quarter car models"
(claytex.com/tech-blog/a-look-inside-vesymas-quarter-car-models/).*

Three identical corners cross the same 50 mm bump at 10 m/s:
- one with an ideal linear damper;
- two with a monotube damper built from hydraulic components (rebound and
  compression valves, a bleed orifice and a gas-charged floating piston), once
  with clean oil and once with 2 % entrained air.

The tyre is an `ElastoGap`, so the wheel can leave the road.

Results:
- The hydraulic damper's force-velocity curve is asymmetric and digressive
  (`clean.damper.F_damper` against `clean.damper.v_damper`).
- Aerated oil adds series compliance, which shows as a hysteresis loop.
- All three bodies return to ride height (0.6 m) within 2 mm.

Assumed values:
- 400 kg quarter body and 40 kg wheel;
- 30 kN/m spring and 250 kN/m tyre;
- 46 mm piston and a 20 bar gas charge.

### Vehicle efficiency sensitivity over NEDC

*Source: Claytex, "Sensitivity testing of vehicle efficiency using VeSyMA drive
cycle" (claytex.com/tech-blog/sensitivity-testing-of-vehicle-efficiency-using-vesyma-drive-cycle/).*

Four copies of an electric car drive 11 back-to-back NEDC cycles (12,980 s):

| Case | Wh/km | vs baseline |
| --- | ---: | ---: |
| Baseline, flat road, still air | 100 | |
| 2 % grade | 192 | +92 % |
| 25 mph wind at 30° yaw | 156 | +56 % |
| Roof box (+50 kg, +0.12 m² drag area) | 109 | +8.9 % |

Crosswind raises drag through the yaw-dependent drag coefficient
`Cd·(1 + k_yaw·β²)`.

Assumed values:
- 1650 kg car, Cd 0.29, frontal area 2.3 m², crr 0.009;
- 350 N·m / 150 kW machine with copper and iron losses;
- 9:1 reduction;
- 75 kWh pack.

### Coast-down and road-load coefficients

*Source: Modelon, "Advancing your vehicle's energy-efficient design"
(modelon.com/blog/advancing-your-vehicles-energy-efficient-design/).*

**CoastDown** lets the car roll from 130 km/h in neutral and terminates at
15 km/h. `tools/fit_coastdown.py` fits the recovered force to
`F0 + F1·v + F2·v²`, giving F0 = 145.7 N, F1 = 0 and F2 = 0.4002 N/(m/s)².
That is exactly the physical rolling resistance and aerodynamic drag, as
expected.

**UDDSEnergy** drives the EPA UDDS cycle twice:
- once with the physical body;
- once with a road-load vehicle that uses the fitted coefficients.

The two energy consumptions differ by **0.007 %**. The UDDS schedule comes from
NREL FASTSim.

## Hydraulic and aerospace

### Landing-gear drop test

*Source: Modelon, "Aircraft landing gear design"
(modelon.com/blog/aircraft-landing-gear-design/).*

This is a CS/FAR 25.723 style drop test. The sprung mass sits on an
oleo-pneumatic strut, with a polytropic gas spring, a quadratic orifice, seal
friction and end stops. The unsprung axle rides on a tyre, and wing lift equals
the weight.

| Sink rate | Load factor |
| ---: | ---: |
| 2.0 m/s | 1.03 |
| 3.05 m/s (limit) | 1.56 |
| 3.66 m/s (reserve energy) | 1.93 |

The strut does not bottom out in the reserve-energy drop. The force-stroke loop
(`limit.strut.F_strut` against `limit.strut.x`) gives a shock-absorber
efficiency near 0.9.

Assumed values:
- 10 t per main gear;
- 150 mm piston and 0.4 m stroke;
- 30 bar precharge and a 5 cm² orifice;
- polytropic exponent 1.3;
- 2 MN/m tyre.

### Aileron actuator

*Source: Modelon, "Modeling and simulation of aircraft aileron actuator"
(modelon.com/blog/modeling-and-simulation-of-aircraft-aileron-actuator/).*

The system:
- a 3000 psi supply with a 50 psi return and 3 m lines;
- a flow-control servo valve with second-order spool dynamics;
- a balanced through-rod cylinder that moves the aileron through a control
  horn, against an aerodynamic hinge moment.

**PositionControl** steps the command to +10° and then to −10°, followed by a
sine. The surface reaches each step within 0.2°.

**SpeedVsLoad** holds the valve fully open and ramps the opposing force to 95 %
of stall force. The rod speed falls monotonically with load and stays below the
ideal square-root valve law, because of line losses, friction and laminar flow
at small openings. It is still moving at 95 % load.

Assumed values:
- 40 mm bore, 20 mm rods, 60 mm stroke;
- a 20 l/min valve at 70 bar with a 100 Hz spool;
- an 80 mm horn.

## Multibody suspension and full vehicle

`library/MultiBodyVehicle` is a separate library built on the Modelica Standard
Library's `Modelica.Mechanics.MultiBody`. It needs MSL 4.1.0 on the source
path:

```sh
MSL=path/to/ModelicaStandardLibrary   # git clone -b v4.1.0 https://github.com/modelica/ModelicaStandardLibrary
./target/release/daedalus test-library $MSL/Modelica $MSL/ModelicaServices $MSL/Complex.mo \
    library/MultiBodyVehicle -m MultiBodyVehicle
```

In the workbench, load the MSL folders and `library/MultiBodyVehicle` with
**Library**. After **Simulate**, the **Animation** view replays the model in
3-D, like Dymola's animation window. It draws every MultiBody shape found in
the result: the default joint, body and spring shapes, and CAD meshes (STL or
OBJ files referenced as `modelica://...` shape types). The library ships meshes
for a treaded tyre, a five-spoke rim, brake disc and caliper, an open-wheel
body and a road bump in `Resources/Shapes` (made by `generate.py` there). The
view has studio lighting, camera presets (Iso, Front, Side, Rear, Top),
follow and chase cameras, click-to-select with X-ray, and records a WebM
video. The visuals do not change the dynamics.

Components:
- `DoubleWishbone`: a short-long-arm corner from hardpoints, with an external
  coil-over on the lower arm and a steerable upright. The loop is closed
  analytically by a `JointRRR`, so a corner adds one state pair and no
  constraint equations.
- `DoubleWishbonePushrod`: the same linkage with an internal spring: a pushrod
  from the lower arm turns a bell-crank rocker that compresses a coil-over
  lying across the chassis. A second `JointRRR` closes the pushrod loop, so it
  still has one state pair.
- `Multilink`: a five-link rear corner (two upper links, two lower links, toe
  link) built elasto-kinematically: the upright is a free body and each link is
  a `CompliantLink`, a stiff axial spring with ball-joint ends (10 kN/mm).
- `WheelPad` and `AeroLoader`: the hydraulic wheel pad and the pull-down
  actuator of a post rig. `Tyre(usePad=true)` stands on a pad, and
  `Vehicle(onRig=true)` puts all four tyres on pads.
- `MacPherson`: a strut on a top-mount hinge with a lower control arm, closed by
  a `RevolutePlanarLoopConstraint`.
- `Tyre`: a point contact that can lift off, with a Pacejka-type lateral force
  and rolling resistance applied at the contact patch.
- `AntiRollBar`: a torsion bar between the left and right lower-arm pivots.
- `Vehicle`: a free 6-DOF sprung body on four double-wishbone corners, with
  front steering, anti-roll bars and a rear-axle speed controller. It starts in
  static equilibrium at design ride height.

| Example | What it shows | States | CPU |
| --- | --- | ---: | ---: |
| SuspensionKinematics | Camber, track change and motion ratio over ±75 mm travel, double wishbone vs MacPherson | 6 | 0.4 s |
| QuarterCarBump | 3-D corner vs a 1-D quarter car with the equivalent wheel rate over a 50 mm bump | 10 | 0.4 s |
| StepSteer | 2° front-wheel step at 72 km/h | 28 | 2.1 s |
| LaneChange | One-period sine steer, 2° peak, at 72 km/h | 28 | 2.6 s |
| BumpLeft | Left wheels cross a 50 mm bump at 36 km/h | 28 | 2.6 s |
| PushrodVsCoilOver | External coil-over vs pushrod, rocker and internal coil-over over ±75 mm | 6 | 0.2 s |
| MultilinkRig | Five-link corner on one wheel pad, ±70 mm bounce | 15 | 0.1 s |
| KCRig | Front axle with pushrod corners and anti-roll bar on two pads: bounce, roll, steer | 14 | 0.7 s |
| SevenPostRig | Full car on four wheel pads and three aero loaders: heave, pitch, roll, warp | 36 | 3.2 s |

Results:
- **Kinematics.** The double wishbone gains 2.2° negative camber at +73 mm
  bump (−21°/m at design) with a near-constant 0.65 motion ratio. The
  MacPherson strut gains 0.7°, with a motion ratio from 0.94 to 1.0.
- **Quarter car.** The 3-D corner and the 1-D model with wheel rate `c·MR²`
  agree to 0.4 mm in peak suspension travel (47 mm) and to 1.5 % in peak tyre
  load (11.5 vs 11.6 kN). Both tyres leave the road behind the crest.
- **Step steer.** The car settles at 14.6°/s yaw rate, 0.52 g and 2.3° roll.
  The linear bicycle model is neutral steer for this tyre, giving `v/L` =
  7.7 1/s; the multibody yaw gain is 7.5 1/s at 0.1° and 7.3 1/s at 2°.
- **Lane change.** The car moves 3.3 m sideways and ends on its original
  heading, with lateral acceleration peaks of ±0.4 g.
- **One-sided bump.** The front-left and rear-left wheels travel 47 and
  52 mm, 0.26 s apart. The body rolls 0.6°, and each left tyre leaves the road
  for about 40 ms behind the crest.
- **Pushrod vs coil-over.** The outboard coil-over has a near-constant 0.65
  motion ratio. The rocker gives a rising rate: 0.70 in full rebound, 0.82 at
  design and 1.00 at full bump, so the wheel rate doubles from 15 to 30 kN/m
  over the travel with one linear spring.
- **Multilink.** −20°/m camber gain (+1.0° at −60 mm, −1.5° at +60 mm), and
  bump steer within 0.05° in kinematics; with the bushes loaded the toe stays
  between −0.44° and +0.06°. Under the static 3.4 kN load the links give
  0.24° of compliance toe-out.
- **K&C rig.** In bounce the bar does not twist and the wheel gains −1.1°
  camber at +43 mm. In roll (±27 mm per side) the bar carries ±475 N·m and the
  pad loads split 5.8 / 2.0 kN.
- **Seven-post rig.** The 3.3 kN of aero load lowers the body 42 mm. The
  12 mm, 1.5 Hz pad bursts excite ±18 mm of body heave, ±0.8° of pitch and
  ±1.8° of roll, all near resonance. Warp barely moves the body (±0.4° roll)
  but swings each tyre load by about ±470 N.

All nine match OpenModelica 1.26.1 running the same models on MSL 4.1.0:
the 98th-percentile difference is below 0.04 % of each signal's range
(below 0.01 % for the four rigs). The
only larger point errors are single samples at tyre lift-off and touchdown.
In OpenModelica the full-car examples take 7 to 16 s of simulation time,
against 1.1 to 2.0 s in DAEdalus (including the animation shapes).

## Note on sources

From the build environment, modelon.com and claytex.com returned HTTP 403.
The examples were therefore built from what the posts describe:
- the system;
- the test procedure;
- the quantities plotted.

They were not checked against the posts' exact figures.
