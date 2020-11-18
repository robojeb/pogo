# What is POGO?

POGO stands for "Online Profile Guided Optimization". 
"But that is not the right order for the acronym!" you might note. 
If UTC can stand for "Coordinated Universal Time", then I feel my acronym can
swap some letters around to sound better.

The purpose of this library is to experiment with optimizing a function at 
run-time. This works by loading an instrumented version of your code as a 
shared object, gathering profiling data, then recompiling with profile guided
optimizations enabled, and finally calling the newer version of the function. 

The library is also designed with an eye for fault-tolerance. 
Should the on-the-fly compilation or optimization fail, the systme will revert
to a version compiled with your project. 

WARNING: At this point in time the library is very experimental. 
My example doesn't even run properly!
Please do not use this anywhere near production. 

## Limitations

Right now this is limited to stand-alone functions with no dependencies. 
All of the functions code has to be able to be compiled without the rest of your
application code available. 

Additionally it doesn't apply any optimizations or handle debug information when
working with the dynamically loaded version of your function

## TODOs

- [ ] Debug why the example isn't working
- [ ] Work out how to support functions linking against the crate
- [ ] Lots of error handling
- [ ] Find better ways of handling the tools that are needed
- [ ] Logging? Or at least remove the current `println!`s 