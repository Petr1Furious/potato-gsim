#include "net/Protocol.hpp"
#include "sim/CommandQueue.hpp"

#include <cassert>
#include <cmath>
#include <cstdlib>
#include <vector>

int main() {
	using sim::AuthoritativeBody;

	{
		std::vector<std::uint8_t> buf;
		std::vector<AuthoritativeBody> bodiesIn;
		bodiesIn.push_back(AuthoritativeBody{
		    .id = 7,
		    .x = 1.0,
		    .y = 2.0,
		    .vx = 3.0,
		    .vy = 4.0,
		    .mass = 5.0,
		    .radius = 6.0,
		    .name = "alpha",
		});
		const std::uint64_t tick = 42;
		const std::uint64_t joinG = 9001;
		assert(net::writeJoinAccept(tick, joinG, bodiesIn, 7, 1.25, buf));

		std::uint64_t tickOut = 0;
		std::uint64_t joinGOut = 0;
		std::vector<AuthoritativeBody> bodiesOut;
		sim::BodyId own = 0;
		double ts = 0.0;
		assert(net::readJoinAccept(buf.data(), buf.size(), tickOut, joinGOut, bodiesOut, own, ts));
		assert(tickOut == tick);
		assert(joinGOut == joinG);
		assert(own == 7);
		assert(std::abs(ts - 1.25) < 1e-12);
		assert(bodiesOut.size() == 1);
		assert(bodiesOut[0].id == 7);
		assert(bodiesOut[0].name == "alpha");
	}

	{
		std::vector<std::uint8_t> buf;
		assert(net::writeShipState(3, 777, 9, 1.1, 2.2, 3.3, 4.4, 0.5f, 1, 0, buf));
		std::uint64_t tick = 0;
		std::uint64_t g = 0;
		sim::BodyId id = 0;
		double px = 0;
		double py = 0;
		double vx = 0;
		double vy = 0;
		float facing = 0.f;
		std::uint8_t tf = 0;
		std::uint8_t tr = 0;
		assert(net::readShipState(buf.data(), buf.size(), tick, g, id, px, py, vx, vy, facing, tf,
		                          tr));
		assert(tick == 3);
		assert(g == 777);
		assert(id == 9);
		assert(std::abs(px - 1.1) < 1e-12);
		assert(tf == 1 && tr == 0);
	}

	{
		std::vector<sim::BodyId> ids{1, 2};
		const double px[2]{10.0, 20.0};
		const double py[2]{-1.0, -2.0};
		const double vx[2]{0.1, 0.2};
		const double vy[2]{0.3, 0.4};
		std::vector<std::uint8_t> buf;
		assert(net::writeWorldDynamicSnapshot(5, 12345, ids, px, py, vx, vy, 2, buf));
		std::uint64_t tick = 0;
		std::uint64_t g = 0;
		std::vector<sim::BodyId> idsOut;
		std::vector<double> pxOut;
		std::vector<double> pyOut;
		std::vector<double> vxOut;
		std::vector<double> vyOut;
		assert(net::readWorldDynamicSnapshot(buf.data(), buf.size(), tick, g, idsOut, pxOut, pyOut,
		                                     vxOut, vyOut));
		assert(tick == 5);
		assert(g == 12345);
		assert(idsOut.size() == 2);
		assert(std::abs(pxOut[1] - 20.0) < 1e-12);
		assert(std::abs(vyOut[0] - 0.3) < 1e-12);
	}

	return EXIT_SUCCESS;
}
