const { Rcon } = require("rcon-client");
const s=(ms)=>new Promise(r=>setTimeout(r,ms));
(async () => {
  const r = await Rcon.connect({host:"192.168.1.93",port:25575,password:"pumpkintest"});
  console.log(await r.send("forceload add -32 -32 32 32"));
  await s(6000);
  for (const c of ["setblock 0 64 0 minecraft:stone","setblock 4 100 4 minecraft:stone","forceload query"]) {
    let out; try { out = await r.send(c); } catch(e){ out="ERR "+e.message; }
    console.log(c.padEnd(38),"=>",String(out).slice(0,120).replace(/\n/g," "));
  }
  r.end();
})();
