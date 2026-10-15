# daedalus_casadi

Optimal control, model predictive control and moving horizon estimation on
Modelica models translated by DAEdalus, with [CasADi](https://web.casadi.org).

```sh
cargo build --release          # the daedalus executable, from the repository root
pip install -e python          # casadi, numpy
python python/examples/ocp_aileron.py --plot aileron.png
```

```python
import daedalus_casadi as dc

m = dc.export("Daedalus.Examples.OptimalControl.AileronServo", smooth=1e-3)
ocp = dc.OCP(m, N=40, free_time=True, T_guess=0.06)
ocp.bound("u", -1, 1)
ocp.subject_to(ocp.final("delta_deg") == 10, ocp.final("aileron.w") == 0)
ocp.minimize(ocp.T)
print(ocp.solve().T)
```

See [docs/optimal-control.md](../docs/optimal-control.md) for the export format,
the API and the three examples (`ocp_aileron.py`, `mpc_active_suspension.py`,
`mhe_coastdown.py`).
