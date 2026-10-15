package Plant
  model FirstOrder "T der(x) = k u - x"
    parameter Real k = 2.5;
    parameter Real T = 0.7;
    input Real u(start = 2);
    output Real y;
    Real x(start = 0, fixed = true);
  equation
    T * der(x) = k * u - x;
    y = x;
    annotation(experiment(StopTime = 3, Interval = 0.01));
  end FirstOrder;

  model Pendulum "damped pendulum with a torque input"
    parameter Real L = 0.8;
    parameter Real g = 9.81;
    parameter Real d = 0.3;
    input Real tau(start = 0);
    output Real xPos;
    Real phi(start = 0.9, fixed = true);
    Real w(start = 0, fixed = true);
  equation
    der(phi) = w;
    L ^ 2 * der(w) = -g * L * sin(phi) - d * w + tau;
    xPos = L * sin(phi);
  end Pendulum;
end Plant;
