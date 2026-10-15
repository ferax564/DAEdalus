within ;
package MultiBodyVehicle "3-D multibody suspension and full-vehicle models built on Modelica.Mechanics.MultiBody"
  extends Modelica.Icons.Package;

  annotation (
    version="0.1.0",
    uses(Modelica(version="4.1.0")),
    Documentation(info="<html>
<p>Automotive examples built from the joints, bodies and line forces of
<code>Modelica.Mechanics.MultiBody</code> (MSL 4.1.0). Everything is plain Modelica: the
library runs in DAEdalus and in OpenModelica with the same results.</p>
<ul>
<li><b>Components.DoubleWishbone</b> and <b>Components.MacPherson</b>: one suspension corner each,
    from hardpoint coordinates. Each is a single-degree-of-freedom planar linkage in the front view
    (y-z plane) of the car. The double wishbone closes its loop analytically with a
    <code>JointRRR</code>; the MacPherson strut closes it with a
    <code>RevolutePlanarLoopConstraint</code>.</li>
<li><b>Components.DoubleWishbonePushrod</b>: the double wishbone with a pushrod, a bell-crank
    rocker and an inboard coil-over (internal spring).</li>
<li><b>Components.Multilink</b>: an elasto-kinematic five-link corner: a free upright held by five
    stiff links (<b>Components.CompliantLink</b>) and a coil-over.</li>
<li><b>Components.WheelPad</b> and <b>Components.AeroLoader</b>: the posts of a seven-post rig.</li>
<li><b>Visuals</b>: animation-only parts (wheel, wishbones, body shell) that draw the CAD meshes in
    <code>Resources/Shapes</code>.</li>
<li><b>Components.Tyre</b>: a point-contact tyre with a vertical spring-damper that can lift off,
    a simplified Pacejka lateral force and rolling resistance, applied at the contact patch.</li>
<li><b>Components.Vehicle</b>: a free 6-DOF sprung body on four double-wishbone corners
    with steerable front uprights, anti-roll bars and a rear-axle speed controller.</li>
</ul>
<p>Coordinates follow ISO 8855: x forward, y left, z up. Gravity acts along -z
(<code>world.n={0,0,-1}</code>). Hardpoints are given for the left-hand corner and mirrored
with <code>side=-1</code>.</p>
<p>Licensed under the Apache License 2.0.</p>
</html>"));
end MultiBodyVehicle;
