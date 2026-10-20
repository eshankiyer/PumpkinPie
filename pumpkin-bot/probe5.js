const { Rcon } = require('rcon-client');
(async () => {
  const r = await Rcon.connect({ host: '192.168.1.93', port: 25575, password: 'pumpkintest' });
  const cmds = [
    'setblock 8 101 8 minecraft:air',
    // does "keep" mode report differently on air vs occupied?
    'setblock 8 101 8 minecraft:glass keep',
    'setblock 8 101 8 minecraft:glass keep',
    'setblock 8 101 8 minecraft:air',
    // what execute subcommands parse at all?
    'execute if block 8 101 8 minecraft:air run seed',
    'execute run seed',
    'execute if block 8 101 8 air run seed',
    '? execute',
  ];
  for (const c of cmds) {
    let o;
    try {
      o = await r.send(c);
    } catch (e) {
      o = 'ERR ' + e.message;
    }
    console.log(c.padEnd(46), '=>', String(o).slice(0, 220).replace(/\n/g, ' | '));
  }
  r.end();
})();
