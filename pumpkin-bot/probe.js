const { Rcon } = require("rcon-client");
(async () => {
  const r = await Rcon.connect({host:"192.168.1.93",port:25575,password:"pumpkintest"});
  for (const c of ["seed","time set day","setblock 2 100 2 minecraft:stone","fill 2 100 2 4 100 4 minecraft:stone","fill 2 100 2 4 100 4 stone","gamerule random_tick_speed 100","gamerule advance_weather"]) {
    let out; try { out = await r.send(c); } catch(e){ out = "ERR "+e.message; }
    console.log(c.padEnd(45), "=>", String(out).slice(0,90).replace(/\n/g," "));
  }
  r.end();
})();
