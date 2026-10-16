within Daedalus;
package Examples "Runnable examples replicating published Modelon and Claytex studies"
  extends Daedalus.Icons.ExamplesPackage;
  annotation (Documentation(info="<html>
<p>Each example re-creates the system studied in a public engineering blog post with the
components of this library. The vendor models and parameter sets behind those posts are not
public, so every example lists the source, what is reproduced, and the parameter values that
were assumed. The aim is the same physics and the same kind of result plot, not identical numbers.</p>
<ul>
<li><b>TorqueConverter</b>: Claytex, <i>Torque converters: the clutches of automatic transmissions</i></li>
<li><b>LandingGear</b>: Modelon, <i>Aircraft landing gear design</i></li>
<li><b>AileronActuator</b>: Modelon, <i>Modeling and simulation of aircraft aileron actuator</i></li>
<li><b>QuarterCar</b>: Claytex, <i>A look inside VeSyMA's quarter car models</i></li>
<li><b>VehicleEfficiency</b>: Claytex, <i>Sensitivity testing of vehicle efficiency using VeSyMA drive cycle</i>;
    Modelon, <i>Advancing your vehicle's energy-efficient design</i></li>
</ul>
<p><b>OptimalControl</b> holds plants with top-level inputs for optimal control, MPC and moving horizon
estimation with CasADi (<code>daedalus export-casadi</code>, <code>python/daedalus_casadi</code>).</p>
<p><b>Embedded</b> holds a DC motor speed loop with discrete and continuous controllers for
embedded C code generation and processor in the loop (<code>daedalus codegen-c</code>,
<code>daedalus hil</code>). <b>SciML</b> holds a motor with unmodelled friction for
missing-physics discovery, symbolic regression and neural surrogates (<code>daedalus.sciml</code>).</p>
</html>"));
end Examples;
