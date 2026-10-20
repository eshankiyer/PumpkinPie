const { Rcon } = require("rcon-client");
(async () => {
  const r = await Rcon.connect({host:"192.168.1.93",port:25575,password:"pumpkintest"});
  for (const c of ["help","forceload add 0 0","setblock 0 64 0 minecraft:stone","list"]) {
    let out; try { out = await r.send(c); } catch(e){ out = "ERR "+e.message; }
    console.log("### "+c+"\n"+String(out).slice(0,700)+"\n");
  }
  r.end();
})();
