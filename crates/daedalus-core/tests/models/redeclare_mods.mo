package RM
  // Modifiers of a redeclared model reach the component, also through a second
  // redeclaration one level down (Modelica.Fluid's `HeatTransfer = HeatTransfer_1`).
  partial model PF
    parameter Real dp_nominal;
    parameter Real k = 1;
  end PF;
  model NF
    extends PF(k = 2);
    Real y = dp_nominal*k;
  end NF;
  model Pipe
    replaceable model FlowModel = PF;
    FlowModel flowModel;
  end Pipe;
  model HX
    replaceable model FM1 = PF;
    Pipe pipe(redeclare model FlowModel = FM1);
  end HX;
  // A package whose `extends` modifies a type it inherits sets that type's start.
  package Base
    type Enthalpy = Real(start = 1);
    model Props
      Enthalpy h;
    end Props;
  end Base;
  package Medium
    extends Base(Enthalpy(start = -400));
  end Medium;
  model Top
    Pipe pipe1(redeclare model FlowModel = NF(dp_nominal = 10));
    HX hx(redeclare model FM1 = NF(dp_nominal = 20, k = 3));
    Medium.Props props;
    Real x(start = 0, fixed = true);
  equation
    // h^2 = 4e4 has the roots +-200: the start value -400 picks the negative one.
    props.h^2 = 4e4;
    der(x) = 1;
    // An assert in an inactive branch does not fire.
    if noEvent(x > 10) then
      assert(x < 5, "inactive branch");
    end if;
  end Top;
end RM;
