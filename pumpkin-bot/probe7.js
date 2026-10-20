const { Rcon } = require('rcon-client');
(async () => {
  const r = await Rcon.connect({ host: '192.168.1.93', port: 25575, password: 'pumpkintest' });
  const run = async (c) => {
    try {
      return String(await r.send(c)).replace(/\n/g, ' | ');
    } catch (e) {
      return 'ERR ' + e.message;
    }
  };
  // lay down a known 3-wide strip: stone, stone, air
  await run('setblock 5 101 5 minecraft:stone');
  await run('setblock 6 101 5 minecraft:stone');
  await run('setblock 7 101 5 minecraft:air');

  console.log('clone filtered stone :', await run('clone 5 101 5 7 101 5 5 103 5 filtered minecraft:stone'));
  console.log('clone filtered air   :', await run('clone 5 101 5 7 101 5 5 105 5 filtered minecraft:air'));
  console.log('fill replace         :', await run('fill 5 101 5 7 101 5 minecraft:diamond_block replace minecraft:stone'));
  console.log('fill plain           :', await run('fill 5 101 5 7 101 5 minecraft:stone'));
  console.log('? fill               :', await run('? fill'));
  console.log('? clone              :', await run('? clone'));
  r.end();
})();
