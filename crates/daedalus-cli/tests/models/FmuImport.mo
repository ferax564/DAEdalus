package FmuImport "Models for the FMU import and SSP tests (crates/daedalus-cli/tests/fmu_import.rs)"
  model Plant "First-order lag: no direct feedthrough"
    parameter Real T = 0.5 "Time constant";
    parameter Real k = 2 "Gain";
    input Real u(start = 0);
    output Real y;
    Real x(start = 0.2, fixed = true);
  equation
    T * der(x) = k * u - x;
    y = x;
    annotation(experiment(StopTime = 5));
  end Plant;

  model Controller "PI controller: direct feedthrough from ym and r to u"
    parameter Real kp = 1.5;
    parameter Real ti = 0.8;
    input Real r(start = 0);
    input Real ym(start = 0);
    output Real u;
    Real xi(start = 0, fixed = true);
  equation
    der(xi) = (r - ym) / ti;
    u = kp * (r - ym + xi);
    annotation(experiment(StopTime = 5));
  end Controller;

  model Reference "Step reference at t = 1"
    parameter Real height = 1;
    output Real r;
  equation
    r = if time < 1 then 0 else height;
    annotation(experiment(StopTime = 5));
  end Reference;

  model Loop "Native closed loop (the reference for the imported FMUs)"
    Plant plant(k = 2.5);
    Controller ctrl(kp = 1.2);
    Reference ref;
  equation
    ctrl.r = ref.r;
    ctrl.ym = plant.y;
    plant.u = ctrl.u;
    annotation(experiment(StopTime = 5, Interval = 0.01));
  end Loop;

  model Ball "Bouncing ball: state events and reinit inside the FMU"
    parameter Real e = 0.8;
    input Real gain(start = 1);
    output Real height;
    Real h(start = 1, fixed = true);
    Real v(start = 0, fixed = true);
  equation
    der(h) = v;
    der(v) = -9.81 * gain;
    when h < 0 then
      reinit(v, -e * pre(v));
    end when;
    height = h;
    annotation(experiment(StopTime = 3));
  end Ball;
end FmuImport;
