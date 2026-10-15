"""CasADi integration for DAEdalus: optimal control, MPC and moving horizon estimation.

``daedalus export-casadi`` writes a model after alias elimination, index
reduction and tearing as a semi-explicit index-1 DAE. This package loads it as
CasADi expressions and transcribes it by direct collocation.

    import daedalus_casadi as dc
    m = dc.export("Daedalus.Examples.OptimalControl.AileronServo", smooth=1e-3)
    ocp = dc.OCP(m, N=40, free_time=True, T_guess=0.06)
    ocp.bound("u", -1, 1)
    ocp.subject_to(ocp.final("delta_deg") == 10, ocp.final("aileron.w") == 0)
    ocp.minimize(ocp.T)
    sol = ocp.solve()
"""

from .export import bundled_library, daedalus_binary, export, simulate_csv
from .mhe import MHE
from .model import Model, Variable, load
from .mpc import MPC
from .ocp import OCP, Solution
from .simulate import Trajectory, simulate

__all__ = [
    "MHE",
    "MPC",
    "OCP",
    "Model",
    "Solution",
    "Trajectory",
    "Variable",
    "bundled_library",
    "daedalus_binary",
    "export",
    "load",
    "simulate",
    "simulate_csv",
]
__version__ = "0.1.0"
