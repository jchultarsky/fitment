import cadquery as cq, sys, os
out = sys.argv[1]
pts = [(-30,-15),(30,-15),(-30,15),(30,15)]
# Plate: z in [-10,0], four tapped holes (tap drill 5.0 for M6)
plate = cq.Workplane("XY").box(100,60,10,centered=(True,True,False)).translate((0,0,-10))
plate = plate.faces(">Z").workplane().pushPoints(pts).hole(5.0)
# Target bracket: z in [0,8], four clearance holes 6.6, one lightening hole 8, one boss
def bracket(L=80,W=50,T=8,d=6.6,pts=pts,extra=True):
    b = cq.Workplane("XY").box(L,W,T,centered=(True,True,False))
    b = b.faces(">Z").workplane().pushPoints(pts).hole(d)
    if extra:
        b = b.faces(">Z").workplane().pushPoints([(0,0)]).hole(8.0)
        b = b.faces(">Z").workplane().center(0,-15).circle(6).extrude(5)
    return b
target = bracket()
bolt = cq.Workplane("XY").circle(3.0).extrude(18).translate((0,0,-10)).union(
       cq.Workplane("XY").circle(5.0).extrude(4).translate((0,0,8)))
asm = cq.Assembly(name="ASM")
asm.add(plate, name="PLATE")
asm.add(target, name="BRACKET")
for i,(x,y) in enumerate(pts):
    asm.add(bolt, name=f"BOLT{i+1}", loc=cq.Location(cq.Vector(x,y,0)))
asm.save(os.path.join(out,"asm.step"))
# Candidates, each modelled in its own arbitrary frame
loc = cq.Location(cq.Vector(200,-40,17), cq.Vector(1,1,0), 37)
def save(w,name): cq.exporters.export(w.val().located(loc), os.path.join(out,name))
save(bracket(L=90,W=44,T=6,extra=False), "cand_good.step")            # different body, same interface
save(bracket(pts=[(-30,-15),(30,-15),(-30,15),(30.5,15)],extra=False), "cand_shift.step")  # one hole 0.5 off
save(bracket(d=5.5,extra=False), "cand_small.step")                    # holes too small for the bolt
save(bracket(pts=[(-30,-15),(30,-15),(-30,15)],extra=False), "cand_missing.step")  # one hole missing
