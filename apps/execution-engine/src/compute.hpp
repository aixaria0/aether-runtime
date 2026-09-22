#pragma once
#include <string>
struct Computation { bool success; std::string output; std::string error; };
Computation compute(const std::string& operation, const std::string& payload);
