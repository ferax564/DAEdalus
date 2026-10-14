#!/usr/bin/env python3
"""Generate a scalability benchmark: a chain of N masses and springs built from the
Daedalus library's translational components, with a stiff end stop.

    python3 tools/gen_chain.py 5000 > /tmp/chain.mo
    daedalus simulate library/Daedalus /tmp/chain.mo -m Chain --stop 1
"""
import sys

n = int(sys.argv[1]) if len(sys.argv) > 1 else 1000
print("model Chain")
print("  import T = Daedalus.Mechanics.Translational;")
print("  T.Components.Fixed fixed;")
print(f"  T.Components.Mass m[{n}](each m=0.1, s(each fixed=true), v(each fixed=true));")
print(f"  T.Components.SpringDamper k[{n}](each c=1e4, each d=0.5);")
print("  T.Sources.Force push;")
print("  Daedalus.Blocks.Sources.Sine f(amplitude=10, f=2);")
print("equation")
print("  connect(fixed.flange, k[1].flange_a);")
print(f"  for i in 1:{n} loop")
print("    connect(k[i].flange_b, m[i].flange_a);")
print("  end for;")
print(f"  for i in 1:{n - 1} loop")
print("    connect(m[i].flange_b, k[i + 1].flange_a);")
print("  end for;")
print(f"  connect(push.flange, m[{n}].flange_b);")
print("  connect(f.y, push.f);")
print("  annotation(experiment(StopTime=1));")
print("end Chain;")
