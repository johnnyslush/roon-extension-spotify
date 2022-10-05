const fs = require('fs');
module.exports = options => {
    const {destination} = options;
    if (!destination) throw '';
    return fs.createWriteStream(options.destination, { flags: 'a'});
};
