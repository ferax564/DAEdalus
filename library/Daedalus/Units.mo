within Daedalus;
package Units "SI types used throughout the library"
  extends Daedalus.Icons.Package;

  type Time = Real(final quantity="Time", final unit="s");
  type Frequency = Real(final quantity="Frequency", final unit="Hz");
  type Length = Real(final quantity="Length", final unit="m");
  type Position = Length;
  type Distance = Length(min=0);
  type Area = Real(final quantity="Area", final unit="m2");
  type Volume = Real(final quantity="Volume", final unit="m3");
  type Velocity = Real(final quantity="Velocity", final unit="m/s");
  type Acceleration = Real(final quantity="Acceleration", final unit="m/s2");
  type Mass = Real(final quantity="Mass", final unit="kg", min=0);
  type Density = Real(final quantity="Density", final unit="kg/m3", displayUnit="g/cm3", min=0);
  type Force = Real(final quantity="Force", final unit="N");
  type TranslationalSpringConstant = Real(final quantity="TranslationalSpringConstant", final unit="N/m");
  type TranslationalDampingConstant = Real(final quantity="TranslationalDampingConstant", final unit="N.s/m");
  type Angle = Real(final quantity="Angle", final unit="rad", displayUnit="deg");
  type AngularVelocity = Real(final quantity="AngularVelocity", final unit="rad/s", displayUnit="rpm");
  type AngularAcceleration = Real(final quantity="AngularAcceleration", final unit="rad/s2");
  type Inertia = Real(final quantity="MomentOfInertia", final unit="kg.m2", min=0);
  type Torque = Real(final quantity="Torque", final unit="N.m");
  type RotationalSpringConstant = Real(final quantity="RotationalSpringConstant", final unit="N.m/rad");
  type RotationalDampingConstant = Real(final quantity="RotationalDampingConstant", final unit="N.m.s/rad");
  type Power = Real(final quantity="Power", final unit="W");
  type Energy = Real(final quantity="Energy", final unit="J");
  type Pressure = Real(final quantity="Pressure", final unit="Pa", displayUnit="bar", nominal=1e5);
  type AbsolutePressure = Pressure(min=0);
  type PressureDifference = Pressure;
  type BulkModulus = Real(final quantity="BulkModulus", final unit="Pa", displayUnit="bar");
  type VolumeFlowRate = Real(final quantity="VolumeFlowRate", final unit="m3/s", displayUnit="l/min");
  type DynamicViscosity = Real(final quantity="DynamicViscosity", final unit="Pa.s", min=0);
  type Voltage = Real(final quantity="Voltage", final unit="V");
  type Current = Real(final quantity="Current", final unit="A");
  type Resistance = Real(final quantity="Resistance", final unit="Ohm");
  type ElectricCharge = Real(final quantity="ElectricCharge", final unit="C");
  type Temperature = Real(final quantity="ThermodynamicTemperature", final unit="K", displayUnit="degC", min=0, nominal=300);
  type Efficiency = Real(final quantity="Efficiency", final unit="1", min=0);
  type DisplacementPerRevolution = Real(final quantity="Displacement", final unit="m3/rad");

  annotation (Documentation(info="<html><p>SI types with units and display units, so that results
show in bar, rpm, deg, l/min and degC in the plot window.</p></html>"));
end Units;
